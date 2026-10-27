//! Nesting-capped JSON parsing into `serde_json::Value` and an iterative walker.
//!
//! serde_json's own recursion limit (128) is stricter than the product's requirement,
//! and its default path deep-recurses before reporting. This
//! module parses with a depth-counting seed instead, so the cap is enforced *during* parsing by
//! the same single pass, and no recursion beyond the cap ever starts.

use serde::de::Error as _;
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::error::{DEPTH_MARKER, Error, Result};

/// Caps applied while parsing a hostile JSON document.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    /// Maximum nesting depth of containers (the root is depth 0; a `[[ ]]]` chain of 256 is
    /// allowed at most). The default caps hostile input at 256.
    pub max_depth: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Self { max_depth: 256 }
    }
}

/// Parse `input` (already fully read by the caller; callers use [`crate::read_file_capped`])
/// into a Value with the caps applied. Object key order is preserved (workspace `preserve_order`).
pub fn parse_json(input: &[u8], caps: &Caps) -> Result<Value> {
    parse_json_inner("JSON input", input, caps)
}

/// Same, for string inputs (cURL bodies, pasted JSON, ...).
pub fn parse_json_str(input: &str, caps: &Caps) -> Result<Value> {
    parse_json_inner("JSON input", input.as_bytes(), caps)
}

fn parse_json_inner(what: &str, input: &[u8], caps: &Caps) -> Result<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    // serde_json's hard-coded recursion limit (128) is tighter than the product cap; the
    // depth cap lives in `Seed`, which refuses to descend past `caps.max_depth` before any
    // deeper parsing starts.
    deserializer.disable_recursion_limit();
    let seed = Seed { max_depth: caps.max_depth, depth: 0 };
    let value = seed.deserialize(&mut deserializer).map_err(|err| {
        if err.to_string().contains(DEPTH_MARKER) {
            Error::DepthExceeded { what: what.to_string(), cap: caps.max_depth }
        } else {
            Error::JsonSyntax { what: what.to_string(), source: err }
        }
    })?;
    deserializer.end().map_err(|source| Error::JsonSyntax { what: what.to_string(), source })?;
    Ok(value)
}

/// The cap-carrying seed: one increment per container level; checked before descending.
#[derive(Clone, Copy)]
struct Seed {
    max_depth: usize,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> std::result::Result<Value, D::Error> {
        // The cap is container-specific: a scalar inside the deepest allowed container is not
        // itself nesting, and shares `depth` with that container's children. `visit_seq` and
        // `visit_map` enforce `depth < max_depth` before parsing their own descendants.
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("any JSON value, at most 256 containers deep")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(serde_json::Number::from(v)))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(serde_json::Number::from(v)))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        // serde_json's own Value behaviour: a non-finite float lowers to Null, it cannot be
        // represented in JSON anyway.
        Ok(serde_json::Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_bytes<E: serde::de::Error>(self, _v: &[u8]) -> std::result::Result<Value, E> {
        Err(E::custom("JSON has no byte array form"))
    }

    fn visit_none<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<Value, S::Error> {
        // This container sits at `depth`; nesting it beyond the cap is the violation.
        if self.depth >= self.max_depth {
            return Err(S::Error::custom(DEPTH_MARKER));
        }
        let mut out = Vec::new();
        let child = Seed { max_depth: self.max_depth, depth: self.depth.checked_add(1).ok_or_else(|| S::Error::custom(DEPTH_MARKER))? };
        while let Some(item) = seq.next_element_seed(child)? {
            out.push(item);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        if self.depth >= self.max_depth {
            return Err(A::Error::custom(DEPTH_MARKER));
        }
        let mut out = serde_json::Map::new();
        let child = Seed { max_depth: self.max_depth, depth: self.depth.checked_add(1).ok_or_else(|| A::Error::custom(DEPTH_MARKER))? };
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(child)?;
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }
}

/// Maximum nesting depth of a Value (a scalar is 0), computed iteratively — safe on any depth
/// a Value could reach, even beyond what the parser admits. Counts containers along the
/// deepest branch, which matches the parser's cap semantics.
pub fn max_depth(value: &Value) -> usize {
    let mut deepest = 0usize;
    let mut stack: Vec<(&Value, usize)> = Vec::new();
    stack.push((value, 0));
    while let Some((node, depth)) = stack.pop() {
        let mut child_depth = depth;
        match node {
            Value::Array(items) => {
                child_depth += 1;
                deepest = deepest.max(child_depth);
                for item in items {
                    stack.push((item, child_depth));
                }
            }
            Value::Object(map) => {
                child_depth += 1;
                deepest = deepest.max(child_depth);
                for item in map.values() {
                    stack.push((item, child_depth));
                }
            }
            _ => {}
        }
    }
    deepest
}

/// Pre-order container walk with an iterative stack and a defensive depth cap. Returns false
/// from `visit` to prune the subtree. `Value`s cannot be cyclic, but the cap still bounds the
/// walk the way AGENTS.md 4 asks every input-bound traversal to be bounded.
pub fn walk(value: &Value, max_depth: usize, visit: &mut dyn FnMut(&Value, usize) -> bool) {
    let mut stack: Vec<(&Value, usize)> = vec![(value, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > max_depth {
            continue;
        }
        if !visit(node, depth) {
            continue;
        }
        match node {
            Value::Array(items) => {
                for item in items.iter().rev() {
                    stack.push((item, depth.saturating_add(1)));
                }
            }
            Value::Object(map) => {
                for item in map.values().rev() {
                    stack.push((item, depth.saturating_add(1)));
                }
            }
            _ => {}
        }
    }
}

/// One step from the document root to the container holding a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathSegment {
    /// The container is the value of this object key.
    Key(String),
    /// The container is an element of an array.
    Index,
}

/// An object key found by [`object_keys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonKey {
    /// Byte range of the key token, quotes included.
    pub range: std::ops::Range<usize>,
    /// The unescaped key.
    pub name: String,
    /// Containers from the root down to (excluding) the object that holds the key.
    pub path: Vec<JsonPathSegment>,
}

/// Containers deeper than this are not scanned for keys.
pub const MAX_SCAN_DEPTH: usize = 64;
/// The scan stops after this many keys.
pub const MAX_SCAN_KEYS: usize = 10_000;

enum ScanFrame {
    Object { entered: Option<JsonPathSegment>, expect_key: bool, last_key: String },
    Array { entered: Option<JsonPathSegment> },
}

impl ScanFrame {
    fn entered(&self) -> Option<&JsonPathSegment> {
        match self {
            ScanFrame::Object { entered, .. } | ScanFrame::Array { entered } => entered.as_ref(),
        }
    }
}

/// Lists every object key in possibly half-typed JSON, with its byte range and path. Purely
/// lexical and tolerant: invalid or unfinished text never fails, `{{variable}}` placeholders
/// outside strings are skipped, an unterminated string ends the scan, unbalanced closers are
/// ignored, nesting past [`MAX_SCAN_DEPTH`] is not descended and at most [`MAX_SCAN_KEYS`] keys
/// are returned.
pub fn object_keys(text: &str) -> Vec<JsonKey> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut stack: Vec<ScanFrame> = Vec::new();
    // Containers opened past the depth cap; their contents are scanned but not recorded.
    let mut overflow: usize = 0;
    let mut i = 0usize;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b'"' => {
                let Some(end) = string_end(bytes, i) else { break };
                let after = skip_whitespace(bytes, end);
                if overflow == 0
                    && bytes.get(after) == Some(&b':')
                    && let Some(ScanFrame::Object { expect_key, last_key, .. }) = stack.last_mut()
                    && *expect_key
                {
                    let quoted = text.get(i..end).unwrap_or_default();
                    let name = serde_json::from_str::<String>(quoted)
                        .unwrap_or_else(|_| text.get(i.saturating_add(1)..end.saturating_sub(1)).unwrap_or_default().to_owned());
                    *expect_key = false;
                    last_key.clone_from(&name);
                    let path = stack.iter().filter_map(ScanFrame::entered).cloned().collect();
                    keys.push(JsonKey { range: i..end, name, path });
                    if keys.len() >= MAX_SCAN_KEYS {
                        break;
                    }
                }
                i = end;
                continue;
            }
            b'{' if bytes.get(i.saturating_add(1)) == Some(&b'{') => {
                i = find_from(bytes, i.saturating_add(2), b"}}").map_or(bytes.len(), |at| at.saturating_add(2));
                continue;
            }
            b'{' | b'[' => {
                if overflow > 0 || stack.len() >= MAX_SCAN_DEPTH {
                    overflow = overflow.saturating_add(1);
                } else {
                    let entered = match stack.last() {
                        Some(ScanFrame::Object { last_key, .. }) => Some(JsonPathSegment::Key(last_key.clone())),
                        Some(ScanFrame::Array { .. }) => Some(JsonPathSegment::Index),
                        None => None,
                    };
                    stack.push(if byte == b'{' {
                        ScanFrame::Object { entered, expect_key: true, last_key: String::new() }
                    } else {
                        ScanFrame::Array { entered }
                    });
                }
            }
            b'}' | b']' => {
                if overflow > 0 {
                    overflow = overflow.saturating_sub(1);
                } else if matches!((stack.last(), byte), (Some(ScanFrame::Object { .. }), b'}') | (Some(ScanFrame::Array { .. }), b']')) {
                    stack.pop();
                }
            }
            b',' => {
                if overflow == 0
                    && let Some(ScanFrame::Object { expect_key, .. }) = stack.last_mut()
                {
                    *expect_key = true;
                }
            }
            _ => {}
        }
        i = i.saturating_add(1);
    }
    keys
}

/// Byte offset just past the closing quote of the string opening at `start`, or `None` when
/// the string is unterminated.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start.saturating_add(1);
    loop {
        match bytes.get(i)? {
            b'\\' => i = i.saturating_add(2),
            b'"' => return Some(i.saturating_add(1)),
            _ => i = i.saturating_add(1),
        }
    }
}

fn skip_whitespace(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i = i.saturating_add(1);
    }
    i
}

fn find_from(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(from..)?.windows(needle.len()).position(|window| window == needle).map(|at| at.saturating_add(from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    fn names(text: &str) -> Vec<(String, Vec<JsonPathSegment>)> {
        object_keys(text).into_iter().map(|key| (key.name, key.path)).collect()
    }

    #[test]
    fn object_keys_reports_ranges_and_paths() {
        let text = r#"{"a": 1, "b": {"c": [ {"d": "x,y:"} ]}}"#;
        let keys = object_keys(text);
        assert_eq!(keys.len(), 4);
        assert_eq!(&text[keys[0].range.clone()], "\"a\"");
        assert_eq!(&text[keys[3].range.clone()], "\"d\"");
        use JsonPathSegment::{Index, Key};
        assert_eq!(
            names(text),
            vec![
                ("a".into(), vec![]),
                ("b".into(), vec![]),
                ("c".into(), vec![Key("b".into())]),
                ("d".into(), vec![Key("b".into()), Key("c".into()), Index]),
            ]
        );
    }

    #[test]
    fn object_keys_tolerates_half_typed_text_and_variables() {
        assert!(object_keys(r#"{"te"#).is_empty(), "unterminated string ends the scan");
        assert_eq!(names(r#"{"n": {{num}}, "m": "{{v}}", "é\"q": 1"#), vec![("n".into(), vec![]), ("m".into(), vec![]), ("é\"q".into(), vec![]),]);
        assert_eq!(names(r#"]]}} {"a": 1"#), vec![("a".into(), vec![])], "stray closers are ignored");
        assert_eq!(names(r#"{"a" 1, "b": 2}"#), vec![("b".into(), vec![])], "a string without a colon is no key");
        assert_eq!(names(r#"{"a": "b": 1}"#), vec![("a".into(), vec![])], "a value is no key");
        let deep = format!("{}{{\"k\":1}}", "[".repeat(MAX_SCAN_DEPTH));
        assert!(object_keys(&deep).is_empty(), "past the depth cap nothing is recorded");
        assert!(object_keys("{{ unterminated").is_empty());
    }

    #[test]
    fn parses_scalars_and_preserves_object_key_order() {
        let v = parse_json(br#"{"zeta":1,"alpha":2}"#, &Caps::default()).unwrap();
        let map = v.as_object().unwrap();
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["zeta", "alpha"], "preserve_order keeps document order");
    }

    #[test]
    fn depth_is_counted_from_the_document_root() {
        assert_eq!(max_depth(&parse_json(br#"[1,{"x":[2]}]"#, &Caps::default()).unwrap()), 3);
        assert_eq!(max_depth(&parse_json(b"42", &Caps::default()).unwrap()), 0);
    }

    #[test]
    fn nesting_cap_is_enforced_during_parse() {
        let deep = build_deep(257);
        let err = parse_json(deep.as_bytes(), &Caps::default()).unwrap_err();
        assert!(matches!(err, Error::DepthExceeded { cap: 256, .. }), "{err}");
        // One level under the cap parses fine.
        let ok = parse_json(build_deep(256).as_bytes(), &Caps::default());
        assert!(ok.is_ok(), "256 should pass: {ok:?}");
    }

    #[test]
    fn depth_cap_error_is_distinguished_from_syntax() {
        let err = parse_json(b"{\"x\": ", &Caps::default()).unwrap_err();
        assert!(matches!(err, Error::JsonSyntax { .. }), "{err}");
    }

    #[test]
    fn low_cap_downgrades_error_kind() {
        let err = parse_json(build_deep(3).as_bytes(), &Caps { max_depth: 2 }).unwrap_err();
        assert!(matches!(err, Error::DepthExceeded { cap: 2, .. }), "{err}");
    }

    #[test]
    fn huge_volumes_do_not_recurse_in_the_walker() {
        // A Value built in memory can exceed what the parser admits; the walker must not
        // recurse. Depth is bounded here because dropping a very deep in-memory Value
        // recurses in serde_json's own Drop (the parser cap is what keeps real docs shallow).
        const DEPTH: usize = 2_000;
        let mut v = Value::Bool(true);
        for _ in 0..DEPTH {
            v = Value::Array(vec![v]);
        }
        assert_eq!(max_depth(&v), DEPTH);
        let mut visited = 0;
        walk(&v, usize::MAX, &mut |_node, _depth| {
            visited += 1;
            true
        });
        assert_eq!(visited, DEPTH + 1);
        let mut pruned = 0;
        walk(&v, 5, &mut |_node, _depth| {
            pruned += 1;
            true
        });
        assert_eq!(pruned, 6, "cap prunes the iteration, never recurses");
    }

    /// A JSON document nested `levels` deep: a chain of objects.
    fn build_deep(levels: usize) -> String {
        let mut s = String::with_capacity(6 * levels);
        for _ in 0..levels {
            s.push_str("{\"x\":");
        }
        s.push('1');
        for _ in 0..levels {
            s.push('}');
        }
        s
    }
}
