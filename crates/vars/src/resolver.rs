//! The resolution engine: scopes in precedence order, recursive expansion, depth and cycle
//! guards. Narrower scopes win (global < collection < folder outer-to-inner <
//! environment < data < local); a scope of the same kind added later wins the tie, so folder
//! scopes are added outer-to-inner.
//!
//! Unresolved `{{name}}` stays as literal text and is reported for the chips; resolution
//! never errors on hostile input, it always terminates.

use crate::MAX_DEPTH;
use crate::dynamic;

/// Where a scope value came from — the scope badge the UI shows on chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeKind {
    #[default]
    Local,
    Data,
    Environment,
    Folder,
    Collection,
    Globals,
}

impl ScopeKind {
    /// Precedence: a larger number wins the lookup.
    pub const fn precedence(&self) -> u8 {
        match self {
            ScopeKind::Globals => 0,
            ScopeKind::Collection => 1,
            ScopeKind::Folder => 2,
            ScopeKind::Environment => 3,
            ScopeKind::Data => 4,
            ScopeKind::Local => 5,
        }
    }
}

/// One scope of the chain. Entries resolve normally; `secrets` lists the keys whose VALUES
/// are masked everywhere they are echoed (console, logs, snippets).
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub kind: ScopeKind,
    pub entries: Vec<(String, String)>,
    pub secrets: Vec<String>,
}

impl Scope {
    pub fn new(kind: ScopeKind, entries: impl IntoIterator<Item = (String, String)>) -> Scope {
        Scope { kind, entries: entries.into_iter().collect(), secrets: Vec::new() }
    }

    /// A scope with `entries` plus secret entries (values resolve; echoes mask).
    pub fn with_secrets(mut self, secrets: impl IntoIterator<Item = (String, String)>) -> Scope {
        for (key, value) in secrets {
            self.entries.push((key.clone(), value));
            self.secrets.push(key);
        }
        self
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn is_secret(&self, key: &str) -> bool {
        self.secrets.iter().any(|k| k == key)
    }

    /// The secret VALUES this scope holds (for `mask_secrets`).
    pub fn secret_values(&self) -> Vec<String> {
        self.secrets.iter().filter_map(|k| self.get(k)).map(|v| v.to_string()).collect()
    }
}

/// The resolution output: resolved text plus the chip spans for resolved and unresolved
/// `{{name}}` occurrences.
#[derive(Debug, Default)]
pub struct Resolution {
    pub text: String,
    pub hits: Vec<VarHit>,
}

impl Resolution {
    /// Names that are still `{{...}}` after resolution (deduped), for the Send-block hint
    /// that lists unresolved names.
    pub fn unresolved_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.hits.iter().filter(|h| h.unresolved).map(|h| h.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Names that resolved from secret scopes (the UI masks with a reveal eye).
    pub fn secret_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.hits.iter().filter(|h| h.is_secret && !h.unresolved).map(|h| h.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// One `{{name}}` occurrence in the resolved text, as the chips draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarHit {
    pub name: String,
    pub source: VarSource,
    /// Set when the value came from a secret scope entry.
    pub is_secret: bool,
    /// Span over the RESOLVED text, byte offsets.
    pub start: usize,
    pub end: usize,
    /// True when the name stayed unresolved (red chip; Send-block warnings).
    pub unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarSource {
    Scope(ScopeKind),
    Dynamic,
}

#[derive(Debug, Clone, Default)]
pub struct Resolver {
    scopes: Vec<Scope>,
}

impl Resolver {
    /// Nothing but dynamic variables.
    pub fn new() -> Resolver {
        Resolver::default()
    }

    pub fn with_scope(mut self, scope: Scope) -> Resolver {
        self.scopes.push(scope);
        self
    }

    /// The scope chain, read-only (masking consults it; nothing may mutate it).
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Highest-precedence scope defining `key`; equal precedence falls to the latest added.
    fn lookup(&self, key: &str) -> Option<&Scope> {
        let mut best: Option<&Scope> = None;
        for scope in &self.scopes {
            if scope.get(key).is_none() {
                continue;
            }
            if best.as_ref().is_none_or(|current| scope.kind.precedence() >= current.kind.precedence()) {
                best = Some(scope);
            }
        }
        best
    }

    /// Resolve a template once.
    pub fn resolve_text(&self, template: &str) -> Resolution {
        let mut text = String::with_capacity(template.len());
        let mut hits = Vec::new();
        self.expand(template, 0, &mut Stackup::default(), &mut text, &mut hits);
        Resolution { text, hits }
    }

    /// Alias other modules and tests use (`Resolver::new().resolve("...")`).
    pub fn resolve(&self, template: &str) -> Resolution {
        self.resolve_text(template)
    }

    /// Expand one template into `out`.
    ///
    /// Cycle guard: expanding `{{name}}` pushes it on `stack`; the name reappearing inside
    /// its own expansion chain leaves the `{{name}}` literal and reports it as unresolved.
    /// Depth cap: while `depth < MAX_DEPTH` a value containing `{{...}}` expands further;
    /// beyond the cap the rest stays literal with a warning hit.
    fn expand(&self, template: &str, depth: usize, stack: &mut Stackup, out: &mut String, hits: &mut Vec<VarHit>) {
        let mut byte_index = 0usize;
        while byte_index < template.len() {
            let rest = &template[byte_index..];
            let Some((open_rel, close_end_rel)) = next_var(rest) else {
                out.push_str(rest);
                return;
            };
            // Text between variables copies through unchanged (the resolution output is the
            // template with each `{{name}}` replaced or left as a literal).
            out.push_str(&rest[..open_rel]);
            let name = &rest[open_rel + 2..close_end_rel - 2];
            let span_start = out.len();
            let raw = self.raw_value(name);
            match raw {
                RawValue::Dynamic(value) => {
                    out.push_str(&value);
                    hits.push(VarHit {
                        name: name.to_string(),
                        source: VarSource::Dynamic,
                        is_secret: false,
                        start: span_start,
                        end: out.len(),
                        unresolved: false,
                    });
                }
                RawValue::FromScope { value, kind, secret } => {
                    let deeper = value.contains("{{");
                    let cycle = stack.contains(name);
                    let within_depth = depth < MAX_DEPTH;
                    match (deeper, cycle, within_depth) {
                        (false, _, _) => {
                            out.push_str(&value);
                            push_hit(hits, name, kind, secret, span_start, out.len(), false);
                        }
                        (true, false, true) => {
                            // Push the protection stack, recurse, then close it.
                            stack.push(name.to_string());
                            self.expand(&value, depth + 1, stack, out, hits);
                            stack.pop();
                            push_hit(hits, name, kind, secret, span_start, out.len(), false);
                        }
                        (true, true, _) => {
                            // Cycle: the mention itself stays literal (the
                            // resolver warns and leaves the text unresolved).
                            out.push_str(&rest[open_rel..close_end_rel]);
                            push_hit(hits, name, kind, secret, span_start, out.len(), true);
                        }
                        (true, false, false) => {
                            // Depth cap: the value lands verbatim and its own `{{...}}` stays.
                            out.push_str(&value);
                            push_hit(hits, name, kind, secret, span_start, out.len(), true);
                        }
                    }
                }
                RawValue::Unresolved => {
                    out.push_str(&rest[open_rel..close_end_rel]);
                    hits.push(VarHit {
                        name: name.to_string(),
                        source: VarSource::Scope(ScopeKind::Local),
                        is_secret: false,
                        start: span_start,
                        end: out.len(),
                        unresolved: true,
                    });
                }
            }
            byte_index += close_end_rel;
        }
    }

    fn raw_value(&self, name: &str) -> RawValue {
        if name.starts_with('$') {
            return match dynamic::try_generate(name) {
                Some(value) => RawValue::Dynamic(value),
                None => RawValue::Unresolved, // unknown dynamic names stay literal (warned)
            };
        }
        match self.lookup(name).and_then(|scope| scope.get(name).map(|value| (scope, value))) {
            Some((scope, value)) => RawValue::FromScope { value: value.to_string(), kind: scope.kind, secret: scope.is_secret(name) },
            None => RawValue::Unresolved,
        }
    }
}

/// Protection stack for the cycle guard (name being expanded along the current chain).
#[derive(Debug, Default)]
struct Stackup {
    names: Vec<String>,
}

impl Stackup {
    fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    fn push(&mut self, name: String) {
        self.names.push(name);
    }

    fn pop(&mut self) {
        self.names.pop();
    }
}

fn push_hit(hits: &mut Vec<VarHit>, name: &str, kind: ScopeKind, secret: bool, start: usize, end: usize, unresolved: bool) {
    hits.push(VarHit { name: name.to_string(), source: VarSource::Scope(kind), is_secret: secret, start, end, unresolved });
}

#[derive(Debug)]
enum RawValue {
    Dynamic(String),
    FromScope { value: String, kind: ScopeKind, secret: bool },
    Unresolved,
}

/// Resolve one template with only dynamic variables (the `replaceIn` entry and tests; the
/// crate re-exports this as `resolve_text`).
pub fn resolve(template: &str) -> Resolution {
    Resolver::new().resolve_text(template)
}

/// Find the FIRST complete `{{name}}` in `text`; (open byte, byte AFTER `}}`), also tolerating
/// an empty name `{{}}` (the caller reports unresolved).
fn next_var(text: &str) -> Option<(usize, usize)> {
    let open = text.find("{{")?;
    let after_open = open + "{{".len();
    let close = text.get(after_open..)?.find("}}")? + after_open;
    Some((open, close + "}}".len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Resolver {
        Resolver::new()
            .with_scope(Scope::new(ScopeKind::Globals, vec![("global".into(), "g".into()), ("both".into(), "from-globals".into())]))
            .with_scope(Scope::new(ScopeKind::Collection, vec![("coll".into(), "c".into()), ("both".into(), "from-collection".into())]))
            .with_scope(Scope::new(ScopeKind::Folder, vec![("folder".into(), "f".into())]))
            .with_scope(Scope::new(ScopeKind::Environment, vec![("env".into(), "e".into()), ("both".into(), "from-env".into())]))
    }

    #[test]
    fn precedence_says_narrowest_wins() {
        let resolution = base().resolve("{{both}} {{env}}");
        assert_eq!(resolution.text, "from-env e");
        assert_eq!(resolution.unresolved_names(), Vec::<&str>::new());
    }

    #[test]
    fn later_added_same_kind_falls_to_the_latest() {
        let r = Resolver::new()
            .with_scope(Scope::new(ScopeKind::Folder, [("who".into(), "outer".into())]))
            .with_scope(Scope::new(ScopeKind::Folder, [("who".into(), "inner".into())]));
        assert_eq!(r.resolve("{{who}}").text, "inner");
    }

    #[test]
    fn unresolved_stay_literals_and_are_listed() {
        let resolution = crate::resolve_text("https://{{missing}}/{{absent}}/ok");
        assert_eq!(resolution.text, "https://{{missing}}/{{absent}}/ok");
        assert_eq!(resolution.unresolved_names(), vec!["absent", "missing"]);
    }

    #[test]
    fn unresolved_list_has_no_duplicates() {
        let resolution = crate::resolve_text("{{missing}}-{{missing}}");
        assert_eq!(resolution.text, "{{missing}}-{{missing}}");
        assert_eq!(resolution.unresolved_names(), vec!["missing"]);
    }

    #[test]
    fn nested_chains_expand_and_spans_track_the_result() {
        let r = Resolver::new()
            .with_scope(Scope::new(ScopeKind::Globals, [("when".into(), "{{whenInner}}".into())]))
            .with_scope(Scope::new(ScopeKind::Environment, [("whenInner".into(), "now".into())]));
        let resolution = r.resolve("at {{when}};");
        assert_eq!(resolution.text, "at now;");
        let hits = &resolution.hits;
        assert_eq!(hits.len(), 2, "inner+outer hits");
        assert_eq!(hits[1].name, "when");
        assert_eq!(&resolution.text[hits[1].start..hits[1].end], "now");
    }

    #[test]
    fn a_two_var_cycle_leaves_the_innermost_literal_and_warns() {
        let r = Resolver::new()
            .with_scope(Scope::new(ScopeKind::Collection, vec![("a".into(), "x{{b}}y".into())]))
            .with_scope(Scope::new(ScopeKind::Globals, vec![("b".into(), "p{{a}}q".into())]));
        let resolution = r.resolve("{{a}}");
        // a expands b (-> x ... y flux), b tries a again (on stack) and STAYS literal.
        assert_eq!(resolution.text, "xp{{a}}qy", "{:?}", resolution.text);
        let unresolved = resolution.unresolved_names();
        assert!(unresolved.contains(&"a"), "{unresolved:?}: cycle surfaced");
    }

    #[test]
    fn deep_chain_hits_the_depth_cap_without_hanging() {
        let mut r = Resolver::new();
        r.scopes.push(Scope::new(ScopeKind::Local, vec![("l0".into(), "leaf".into())]));
        for level in 1..=25 {
            r.scopes.push(Scope::new(ScopeKind::Local, vec![(format!("l{level}"), format!("{{{{l{}}}}}", level - 1))]));
        }
        r.scopes.push(Scope::new(ScopeKind::Local, vec![("start".into(), "{{l25}}".into())]));
        let resolution = r.resolve("#{{start}}#");
        // Expansion stops at depth 10: the value reached there lands verbatim with its own
        // `{{...}}` remaining (leaf is unreachable without re-running Send — spec'd in 5.2).
        assert_eq!(resolution.text, "#{{l15}}#", "{:?}", resolution.text);
    }

    #[test]
    fn dynamic_variables_resolve_fresh_per_occurrence() {
        let resolution = crate::resolve_text("one {{$timestamp}} two {{$timestamp}}");
        let first: Vec<&VarHit> = resolution.hits.iter().take(2).collect();
        let a = &resolution.text[first[0].start..first[0].end];
        let b = &resolution.text[first[1].start..first[1].end];
        assert!(a.parse::<i64>().is_ok() && b.parse::<i64>().is_ok());
        assert!(b.parse::<i64>().expect("stamp") <= a.parse::<i64>().expect("stamp") + 2);
    }

    #[test]
    fn secrets_flag_their_hits() {
        let r = Resolver::new()
            .with_scope(Scope::new(ScopeKind::Environment, [("plain".into(), "visible".into())]).with_secrets([("token".into(), "hunter2".into())]));
        let resolution = r.resolve("{{plain}} {{token}}");
        assert_eq!(resolution.text, "visible hunter2");
        assert_eq!(resolution.secret_names(), vec!["token"]);
        assert!(!resolution.hits[0].is_secret);
        assert!(resolution.hits[1].is_secret);
    }
}
