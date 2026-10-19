//! Secret masking (PROMPT.md 5.2, 5.5, 9): secret variable values never appear verbatim in
//! the console, logs, snippets or history echoes. `mask_secrets` replaces every occurrence
//! while the non-secret text stays byte-identical, so callers can still eyeball requests.

use crate::resolver::{Resolver, Scope};

/// Replace occurrences of the secret VALUES in `text` with a fixed mask glyph block
/// (length-preserving so layout stays readable). Secrets sort longest-first so a value that
/// CONTAINS another still masks the longest match. Matching is case-sensitive on the exact
/// value; empty values mask nothing (they carry no leak risk by themselves).
pub fn mask_secrets(text: &str, scopes: &[Scope]) -> String {
    let values: Vec<String> = scopes.iter().flat_map(|scope| scope.secret_values()).collect();
    let mut sorted: Vec<String> = values;
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for secret in sorted {
        if secret.is_empty() {
            continue;
        }
        out = out.replace(&secret, &mask_of(&secret));
    }
    out
}

/// Mask the secrets inside every resolution: the two helpers callers chain after `resolve`.
pub fn mask_resolution(text: &str, resolver: &Resolver) -> String {
    mask_secrets(text, resolver.scopes())
}

/// The masked form: one block glyph per character (so the mask reads the same length
/// visually), capped at 16 blocks; empty secrets mask empty.
pub fn mask_of(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    "•".repeat(secret.chars().count().min(16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::ScopeKind;

    #[test]
    fn mask_secrets_replaces_exactly_the_value() {
        let scopes = [Scope::new(ScopeKind::Environment, Vec::new()).with_secrets([("token".into(), "hunter2".into())])];
        let request = "Authorization: Bearer hunter2 and header X-T: hunter2extra!";
        let masked = mask_secrets(request, &scopes);
        assert_eq!(masked, "Authorization: Bearer ••••••• and header X-T: •••••••extra!");
    }

    #[test]
    fn longest_secret_wins_when_values_nest() {
        let scopes = [Scope::new(ScopeKind::Environment, Vec::new()).with_secrets([("short".into(), "abc".into()), ("long".into(), "abcde".into())])];
        let masked = mask_secrets("abc abcde abcd", &scopes);
        assert_eq!(masked, "••• ••••• •••d", "longest value masks first; abcd keeps the d after the abc run");
    }

    #[test]
    fn resolution_then_mask_rounds_out() {
        let resolver = Resolver::new().with_scope(Scope::new(ScopeKind::Environment, Vec::new()).with_secrets([("apikey".into(), "s3cr3t!".into())]));
        let resolution = resolver.resolve_text("use {{apikey}} now");
        assert_eq!(resolution.text, "use s3cr3t! now");
        let masked = mask_resolution(&resolution.text, &resolver);
        assert_eq!(masked, "use ••••••• now");
    }
}
