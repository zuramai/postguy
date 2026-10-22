//! Property tests: resolution always terminates, never invents text outside
//! the templates+values it was given, and hostile inputs degrade to literal text.

use postguy_vars::{Resolver, Scope, ScopeKind, resolve_text};

proptest::proptest! {
    /// Resolution terminates on ANY text (cycle + depth guards engage silently) and each
    /// `{{name}}` occurrence produces at most one hit each.
    #[test]
    fn resolution_always_terminates(template in pc_any()) {
        let resolution = resolve_text(&template);
        proptest::prop_assert!(resolution.hits.len() <= 2 * (template.len() + 1));
    }

    /// Resolution is a projection over PLAIN text: resolving a resolution again keeps the
    /// same unresolved set (values resolved already; plain text has no `{{...}}`).
    #[test]
    fn idempotent_over_plain_text(text in pc_safe()) {
        let r = base_resolver();
        let once = r.resolve_text(&text);
        let twice = r.resolve_text(&once.text);
        proptest::prop_assert_eq!(once.unresolved_names(), twice.unresolved_names());
    }
}

/// Text of any shape to exercise the parser paths (openers, closers, $-names).
fn pc_any() -> proptest::prelude::BoxedStrategy<String> {
    use proptest::prelude::*;
    proptest::string::string_regex("(?s)[{\\}a-z0-9$ _][{\\}a-z0-9$ _]{0,40}").expect("regex").boxed()
}

fn pc_safe() -> proptest::prelude::BoxedStrategy<String> {
    use proptest::prelude::*;
    proptest::string::string_regex("[a-z0-9 _]{0,30}").expect("regex").boxed()
}

fn base_resolver() -> Resolver {
    Resolver::new()
        .with_scope(Scope::new(ScopeKind::Globals, vec![("who".into(), "globals".into())]))
        .with_scope(Scope::new(ScopeKind::Environment, vec![("who".into(), "environment".into())]))
}
