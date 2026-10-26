//! Body shortcodes: `{{% base64(file('lib/a.jpg')) %}}`. Pure parsing and evaluation; asset
//! bytes arrive through a caller-supplied loader, so this module never touches the disk.
//!
//! Grammar: a shortcode is `{{%` ws `call` ws `%}}`; `call := ident '(' [arg (',' arg)*] ')'`;
//! `arg := call | string`; a string is `'…'` or `"…"` with the escapes `\\`, `\'` and `\"`;
//! `ident` is `[a-z0-9]+`. Limits: nesting depth 8, 4,096 bytes per shortcode, 1,000 per text.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use md5::Md5;
use sha2::{Digest, Sha256};

/// Deepest call nesting a shortcode may use.
pub const MAX_NESTING: usize = 8;
/// Longest shortcode, in bytes, `{{%` and `%}}` included.
pub const MAX_SHORTCODE_BYTES: usize = 4096;
/// Most shortcodes one text may hold.
pub const MAX_SHORTCODES: usize = 1000;
/// Cap on the expanded text, so nested loads cannot multiply into memory exhaustion.
const MAX_OUTPUT_BYTES: usize = 256 << 20;

const OPEN: &str = "{{%";
const CLOSE: &str = "%}}";
const FUNCTIONS: &str = "file, text, base64, base64url, hex, sha256, md5";

/// One `{{% … %}}` found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcodeSpan {
    /// Byte offset of `{{%`.
    pub start: usize,
    /// Byte offset after `%}}` (or after the bad region for an invalid shortcode).
    pub end: usize,
    /// `Ok` carries the literal asset paths `file()`/`text()` reference (paths containing
    /// `{{vars}}` cannot be checked and are left out); `Err` is why it does not parse.
    pub check: Result<Vec<String>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Str(String),
    Call { name: String, args: Vec<Node>, src: (usize, usize) },
}

#[derive(Debug)]
enum Value {
    Bytes(Vec<u8>),
    Text(String),
}

impl Value {
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Value::Bytes(bytes) => bytes,
            Value::Text(text) => text.into_bytes(),
        }
    }
}

struct Parsed {
    start: usize,
    end: usize,
    node: Result<Node, String>,
}

/// Parse every shortcode in `text`; no I/O. Used by the UI to colour them.
pub fn scan(text: &str) -> Vec<ShortcodeSpan> {
    let (parsed, _) = scan_all(text);
    parsed
        .into_iter()
        .map(|item| {
            let check = item.node.map(|node| {
                let mut paths = Vec::new();
                collect_paths(&node, &mut paths);
                paths
            });
            ShortcodeSpan { start: item.start, end: item.end, check }
        })
        .collect()
}

/// Expand every shortcode in `text`. `resolve_arg` resolves `{{vars}}` inside string
/// arguments; `load` reads an asset by its collection-relative path.
pub fn expand(text: &str, resolve_arg: &dyn Fn(&str) -> String, load: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<String, String> {
    let (parsed, truncated) = scan_all(text);
    if truncated {
        return Err(format!("a text may hold at most {MAX_SHORTCODES} shortcodes"));
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for item in parsed {
        out.push_str(text.get(cursor..item.start).unwrap_or_default());
        cursor = item.end;
        let at = item.start;
        let node = item.node.map_err(|msg| format!("{msg} (shortcode at byte {at})"))?;
        let rendered = render_top(text, &node, resolve_arg, load).map_err(|msg| format!("{msg} (shortcode at byte {at})"))?;
        if out.len().saturating_add(rendered.len()) > MAX_OUTPUT_BYTES {
            return Err(format!("shortcode output is larger than {} MiB", MAX_OUTPUT_BYTES >> 20));
        }
        out.push_str(&rendered);
    }
    out.push_str(text.get(cursor..).unwrap_or_default());
    Ok(out)
}

/// When the whole trimmed text is exactly one `{{% file('p') %}}`, the (still unresolved)
/// path. A single malformed shortcode filling the text is `Some(Err)`; anything else is `None`.
pub fn single_file(text: &str) -> Option<Result<String, String>> {
    let trimmed = text.trim();
    if !trimmed.starts_with(OPEN) {
        return None;
    }
    let (mut parsed, _) = scan_all(trimmed);
    if parsed.len() != 1 {
        return None;
    }
    let item = parsed.pop()?;
    if item.start != 0 || item.end != trimmed.len() {
        return None;
    }
    match item.node {
        Err(msg) => Some(Err(msg)),
        Ok(Node::Call { name, mut args, .. }) if name == "file" => match args.pop() {
            Some(Node::Str(path)) => Some(Ok(path)),
            _ => None,
        },
        Ok(_) => None,
    }
}

fn collect_paths(node: &Node, paths: &mut Vec<String>) {
    if let Node::Call { name, args, .. } = node {
        if (name == "file" || name == "text")
            && let Some(Node::Str(path)) = args.first()
            && !path.contains("{{")
            && !paths.contains(path)
        {
            paths.push(path.clone());
        }
        for arg in args {
            collect_paths(arg, paths);
        }
    }
}

/// Every shortcode in order, plus whether the per-text cap cut the list short.
fn scan_all(text: &str) -> (Vec<Parsed>, bool) {
    let mut found = Vec::new();
    let mut from = 0usize;
    while let Some(offset) = text.get(from..).and_then(|rest| rest.find(OPEN)) {
        let start = from + offset;
        if found.len() >= MAX_SHORTCODES {
            return (found, true);
        }
        let window_end = floor_boundary(text, start.saturating_add(MAX_SHORTCODE_BYTES).min(text.len()));
        let window = text.get(..window_end).unwrap_or(text);
        let mut parser = Parser { text: window, pos: start + OPEN.len() };
        match parser.shortcode() {
            Ok(node) => {
                let end = parser.pos;
                found.push(Parsed { start, end, node: Ok(node) });
                from = end;
            }
            Err(message) => {
                let truncated_window = window_end < text.len();
                let message = if truncated_window && parser.at_window_end() {
                    format!("shortcode is longer than {MAX_SHORTCODE_BYTES} bytes or never closes with `%}}}}`")
                } else {
                    message
                };
                // The bad region runs to the closer when there is one, else to the line end.
                let tail = text.get(start..window_end).unwrap_or_default();
                let end = match tail.find(CLOSE) {
                    Some(close) => start + close + CLOSE.len(),
                    None => start + tail.find('\n').unwrap_or(tail.len()).max(OPEN.len()),
                };
                found.push(Parsed { start, end, node: Err(message) });
                from = end;
            }
        }
    }
    (found, false)
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn at_window_end(&self) -> bool {
        self.pos >= self.text.len()
    }

    fn rest(&self) -> &str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    fn skip_ws(&mut self) {
        let trimmed = self.rest().trim_start();
        self.pos = self.text.len() - trimmed.len();
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn shortcode(&mut self) -> Result<Node, String> {
        self.skip_ws();
        let node = self.call(1)?;
        self.skip_ws();
        if self.rest().starts_with(CLOSE) {
            self.pos += CLOSE.len();
            Ok(node)
        } else if self.at_window_end() {
            Err("shortcode is missing its closing `%}}`".to_string())
        } else {
            Err(format!("expected `%}}}}` after the call, found `{}`", self.peek().unwrap_or(' ')))
        }
    }

    fn call(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_NESTING {
            return Err(format!("shortcodes may nest at most {MAX_NESTING} calls deep"));
        }
        let begin = self.pos;
        let ident_len = self.rest().bytes().take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).count();
        if ident_len == 0 {
            return Err(match self.peek() {
                Some(other) => format!("expected a function name, found `{other}`; available: {FUNCTIONS}"),
                None => "shortcode is missing its function".to_string(),
            });
        }
        let name = self.rest().get(..ident_len).unwrap_or_default().to_string();
        self.pos += ident_len;
        if self.peek() != Some('(') {
            return Err(format!("expected `(` after `{name}`"));
        }
        self.pos += 1;
        let mut args = Vec::new();
        self.skip_ws();
        if self.peek() == Some(')') {
            self.pos += 1;
        } else {
            loop {
                self.skip_ws();
                args.push(self.arg(depth)?);
                self.skip_ws();
                match self.peek() {
                    Some(',') => self.pos += 1,
                    Some(')') => {
                        self.pos += 1;
                        break;
                    }
                    Some(other) => return Err(format!("expected `,` or `)` in {name}(), found `{other}`")),
                    None => return Err(format!("{name}( is missing its closing `)`")),
                }
            }
        }
        validate(&name, &args)?;
        Ok(Node::Call { name, args, src: (begin, self.pos) })
    }

    fn arg(&mut self, depth: usize) -> Result<Node, String> {
        match self.peek() {
            Some(quote @ ('\'' | '"')) => self.string(quote).map(Node::Str),
            Some(_) => self.call(depth + 1),
            None => Err("shortcode ended inside an argument list".to_string()),
        }
    }

    fn string(&mut self, quote: char) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err("unterminated string in shortcode".to_string());
            };
            self.pos += ch.len_utf8();
            if ch == quote {
                return Ok(out);
            }
            if ch != '\\' {
                out.push(ch);
                continue;
            }
            match self.peek() {
                Some(escaped @ ('\\' | '\'' | '"')) => {
                    self.pos += 1;
                    out.push(escaped);
                }
                Some(other) => return Err(format!("unknown escape `\\{other}` in shortcode string; use \\\\, \\' or \\\"")),
                None => return Err("unterminated string in shortcode".to_string()),
            }
        }
    }
}

fn validate(name: &str, args: &[Node]) -> Result<(), String> {
    match name {
        "file" | "text" => {
            if args.len() != 1 {
                return Err(format!("{name}() takes 1 argument"));
            }
            if !matches!(args.first(), Some(Node::Str(_))) {
                return Err(format!("{name}() takes a quoted asset path"));
            }
            Ok(())
        }
        "base64" | "base64url" | "hex" | "sha256" | "md5" => {
            if args.len() == 1 {
                Ok(())
            } else {
                Err(format!("{name}() takes 1 argument"))
            }
        }
        other => Err(format!("unknown shortcode function `{other}`; available: {FUNCTIONS}")),
    }
}

fn render_top(
    text: &str,
    node: &Node,
    resolve_arg: &dyn Fn(&str) -> String,
    load: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<String, String> {
    match eval(node, resolve_arg, load)? {
        Value::Text(text) => Ok(text),
        Value::Bytes(bytes) => String::from_utf8(bytes).map_err(|_| {
            let source = match node {
                Node::Call { src, .. } => text.get(src.0..src.1).unwrap_or("file(…)").to_string(),
                Node::Str(_) => "the value".to_string(),
            };
            format!("{source} is binary; wrap it as base64({source})")
        }),
    }
}

fn eval(node: &Node, resolve_arg: &dyn Fn(&str) -> String, load: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Value, String> {
    let (name, args) = match node {
        Node::Str(raw) => return Ok(Value::Text(resolve_arg(raw))),
        Node::Call { name, args, .. } => (name.as_str(), args),
    };
    let first = args.first().ok_or_else(|| format!("{name}() takes 1 argument"))?;
    match name {
        "file" | "text" => {
            let Node::Str(raw) = first else { return Err(format!("{name}() takes a quoted asset path")) };
            let path = resolve_arg(raw);
            let bytes = load(&path).map_err(|err| format!("{name}('{path}'): {err}"))?;
            if name == "file" {
                Ok(Value::Bytes(bytes))
            } else {
                String::from_utf8(bytes).map(Value::Text).map_err(|_| format!("text('{path}') is not UTF-8 text; use base64(file('{path}'))"))
            }
        }
        _ => {
            let bytes = eval(first, resolve_arg, load)?.into_bytes();
            let rendered = match name {
                "base64" => STANDARD.encode(&bytes),
                "base64url" => URL_SAFE_NO_PAD.encode(&bytes),
                "hex" => hex(&bytes),
                "sha256" => hex(&Sha256::digest(&bytes)),
                "md5" => hex(&Md5::digest(&bytes)),
                other => return Err(format!("unknown shortcode function `{other}`; available: {FUNCTIONS}")),
            };
            Ok(Value::Text(rendered))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            out.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Result<String, String> {
        expand(text, &|arg| arg.replace("{{dir}}", "lib"), &|path| match path {
            "lib/a.bin" => Ok(vec![0xff, 0x00, 0x10]),
            "lib/t.txt" => Ok(b"hello".to_vec()),
            other => Err(format!("asset `{other}` does not exist")),
        })
    }

    #[test]
    fn nested_calls_and_both_quote_styles() {
        assert_eq!(run("a {{% base64(file('lib/a.bin')) %}} b").as_deref(), Ok("a /wAQ b"));
        assert_eq!(run("{{%base64(text(\"lib/t.txt\"))%}}").as_deref(), Ok("aGVsbG8="));
        assert_eq!(run("{{% hex(file('lib/a.bin')) %}}").as_deref(), Ok("ff0010"));
        assert_eq!(run("{{% base64url(file('lib/a.bin')) %}}").as_deref(), Ok("_wAQ"));
        assert_eq!(run("{{% sha256('abc') %}}").as_deref(), Ok("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(run("{{% md5('abc') %}}").as_deref(), Ok("900150983cd24fb0d6963f7d28e17f72"));
        assert_eq!(run("{{% text('lib/t.txt') %}}").as_deref(), Ok("hello"));
    }

    #[test]
    fn variables_inside_arguments_are_resolved_by_the_caller() {
        assert_eq!(run("{{% text('{{dir}}/t.txt') %}}").as_deref(), Ok("hello"));
    }

    #[test]
    fn errors_name_the_problem() {
        assert!(run("{{% foo('x') %}}").expect_err("unknown").contains("unknown shortcode function `foo`; available: file"));
        assert!(run("{{% base64() %}}").expect_err("arity").contains("base64() takes 1 argument"));
        assert!(run("{{% base64('a', 'b') %}}").expect_err("arity").contains("base64() takes 1 argument"));
        assert!(run("{{% text('abc) %}}").expect_err("unterminated").contains("unterminated string"));
        assert!(run("{{% file('lib/a.bin') %}}").expect_err("binary").contains("is binary; wrap it as base64(file('lib/a.bin'))"));
        assert!(run("{{% text('lib/a.bin') %}}").expect_err("utf8").contains("not UTF-8"));
        assert!(run("{{% text('nope') %}}").expect_err("missing").contains("text('nope'): asset `nope` does not exist"));
        assert!(run("{{% base64('a') ").expect_err("unclosed").contains("closing"));
    }

    #[test]
    fn depth_and_length_limits() {
        let deep = |n: usize| format!("{{{{% {}'x'{} %}}}}", "base64(".repeat(n), ")".repeat(n));
        assert!(run(&deep(MAX_NESTING)).is_ok());
        assert!(run(&deep(MAX_NESTING + 1)).expect_err("deep").contains("nest at most 8"));
        let long = format!("{{{{% base64('{}') %}}}}", "x".repeat(MAX_SHORTCODE_BYTES));
        assert!(run(&long).expect_err("long").contains("longer than 4096"));
        let many = "{{% hex('a') %}}".repeat(MAX_SHORTCODES + 1);
        assert!(run(&many).expect_err("many").contains("at most 1000"));
    }

    #[test]
    fn scan_lists_referenced_assets_and_marks_invalid_spans() {
        let text = "x {{% base64(file('a.jpg')) %}} y {{% nope() %}} {{% text('{{dir}}/b') %}}";
        let spans = scan(text);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].check, Ok(vec!["a.jpg".to_string()]));
        assert_eq!(text.get(spans[0].start..spans[0].end), Some("{{% base64(file('a.jpg')) %}}"));
        assert!(spans[1].check.is_err());
        assert_eq!(spans[2].check, Ok(Vec::new()));
    }

    #[test]
    fn single_file_matches_only_a_whole_file_call() {
        assert_eq!(single_file("  {{% file('x/y.png') %}} "), Some(Ok("x/y.png".to_string())));
        assert_eq!(single_file("{{% file('{{d}}/y.png') %}}"), Some(Ok("{{d}}/y.png".to_string())));
        assert_eq!(single_file("a {{% file('y') %}}"), None);
        assert_eq!(single_file("{{% base64(file('y')) %}}"), None);
        assert_eq!(single_file("plain/path.png"), None);
        assert!(matches!(single_file("{{% file( %}}"), Some(Err(_))));
    }

    #[test]
    fn escapes_in_strings() {
        let out = expand("{{% text('a\\'b') %}}", &|s| s.to_string(), &|p| Ok(p.as_bytes().to_vec()));
        assert_eq!(out.as_deref(), Ok("a'b"));
        assert!(run("{{% text('a\\q') %}}").expect_err("escape").contains("unknown escape"));
    }
}
