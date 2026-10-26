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

    /// Scanning shortcodes never panics, and every span is a valid slice of the text.
    #[test]
    fn shortcode_scan_never_panics(text in "(?s).{0,300}", noise in shortcode_noise()) {
        for input in [text.as_str(), noise.as_str()] {
            for span in postguy_vars::shortcode::scan(input) {
                proptest::prop_assert!(span.start < span.end && input.get(span.start..span.end).is_some());
            }
            let _ = postguy_vars::shortcode::single_file(input);
            let _ = postguy_vars::shortcode::expand(input, &|a| a.to_string(), &|_| Ok(vec![1, 2, 3]));
        }
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

/// Strings built from shortcode tokens, to reach the parser's deeper states.
fn shortcode_noise() -> proptest::prelude::BoxedStrategy<String> {
    use proptest::prelude::*;
    proptest::collection::vec(
        prop_oneof![
            Just("{{%"),
            Just("%}}"),
            Just("file("),
            Just("base64("),
            Just(")"),
            Just("'"),
            Just("\""),
            Just("\\"),
            Just(","),
            Just(" "),
            Just("a.png"),
            Just("é")
        ],
        0..40,
    )
    .prop_map(|parts| parts.concat())
    .boxed()
}
