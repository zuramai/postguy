//! Property tests: parse and build are inverses for ANY text, and the param
//! view round-trips; encoding is idempotent and decode(encode(x)) == x over a wide charset.

use postguy_urlkit::*;

proptest::proptest! {
    /// Parse+build preserves the text byte-for-byte: the ONLY safe contract for a field the
    /// user types in and the engine sends verbatim.
    #[test]
    fn build_inverts_parse(text in pc_any()) {
        let url = Url::parse(&text);
        proptest::prop_assert_eq!(url.build(), text);
    }

    /// The edited param view, rebuilt, parses back to exactly those params.
    #[test]
    fn params_rebuilt_round_trip(pairs in pc_pairs()) {
        let params: Vec<Param> = pairs;
        if let Some(q) = query_of_params(&params) {
            proptest::prop_assert_eq!(params_of_query(&q.0), params);
        }
    }

    /// Encoding per component is idempotent: escaping already-escaped text changes nothing.
    #[test]
    fn encoding_is_idempotent(text in pc_ascii()) {
        let once = encode_query(&text);
        proptest::prop_assert_eq!(encode_query(&once), once);
        let once_path = encode_path(&text);
        proptest::prop_assert_eq!(encode_path(&once_path), once_path);
    }

    /// Decode(encode(x)) == x for text WITHOUT percent escapes (spaces and punctuation all
    /// survive). Input WITH `%` escapes has a lenient decode (`%XX` that is not valid UTF-8
    /// shows U+FFFD); that view is for display, and the model's raw query string is what the
    /// wire uses — see the `decode_is_lenient` unit test.
    #[test]
    fn decode_inverts_encoding_over_safe_charset(text in pc_ascii()) {
        let encoded = encode_query(&text);
        proptest::prop_assert_eq!(decode_percent(&encoded), text);
    }
}

/// Any text, including whitespace, weird separators and percent escapes.
fn pc_any() -> proptest::prelude::BoxedStrategy<String> {
    use proptest::prelude::*;
    proptest::string::string_regex("(?s).{0,60}").expect("regex").boxed()
}

/// Up to 6 (key, value) pairs from the round-trip-safe charset (no `%`, so decode/encode are
/// exact inverses for every pair).
fn pc_pairs() -> proptest::prelude::BoxedStrategy<Vec<Param>> {
    use proptest::prelude::*;
    const TOKEN_REGEX: &str = "[a-zA-Z0-9 _=;&./:]{0,12}";
    let pair = (proptest::string::string_regex(TOKEN_REGEX).expect("regex"), proptest::string::string_regex(TOKEN_REGEX).expect("regex"));
    proptest::collection::vec(pair, 0..6).prop_map(|pairs| pairs.into_iter().map(|(k, v)| Param { key: k, value: v }).collect()).boxed()
}

fn pc_ascii() -> proptest::prelude::BoxedStrategy<String> {
    use proptest::prelude::*;
    // Printable ASCII minus `%` (the second range would have re-added it): the charset where
    // decode∘encode is a provable identity; see lenient-decode semantics for `%XX` bytes.
    proptest::string::string_regex("[!-$&-~\\x20]{0,40}").expect("regex").boxed()
}
