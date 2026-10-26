//! A parser for the Bru markup language, written from the public language description
//! (<https://docs.usebruno.com/bru-lang/language>): a file is a list of blocks, and a block is a
//! dictionary (`key: value` lines), a text block (free text) or an array block (a list of
//! strings). Any dictionary key or array item may be prefixed with `~` to disable it.
//!
//! The parser is tolerant of stray lines between blocks, strict about unclosed blocks, and
//! capped on size and block count, so hostile input produces an error and never a panic.

use crate::error::SpecError;

/// Largest `.bru` file the parser accepts.
pub const MAX_FILE_BYTES: usize = 5 << 20;
/// Most blocks one file may hold.
pub const MAX_BLOCKS: usize = 2_000;

/// A parsed `.bru` file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BruDoc {
    /// Blocks in file order.
    pub blocks: Vec<Block>,
}

/// One block: its tag (`meta`, `post`, `body:json`, `vars:secret`, ...) and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub tag: String,
    pub body: BlockBody,
}

/// The three block kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockBody {
    Dict(Vec<DictEntry>),
    Text(String),
    /// Items keep a leading `~` when disabled.
    List(Vec<String>),
}

/// One `key: value` line of a dictionary block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictEntry {
    pub key: String,
    pub value: String,
    pub enabled: bool,
    /// From a preceding `@description('''...''')` annotation.
    pub description: Option<String>,
}

impl BruDoc {
    /// The first block with this tag.
    pub fn block(&self, tag: &str) -> Option<&Block> {
        self.blocks.iter().find(|block| block.tag == tag)
    }

    /// The entries of the dictionary block `tag`.
    pub fn dict(&self, tag: &str) -> Option<&[DictEntry]> {
        match self.block(tag).map(|block| &block.body) {
            Some(BlockBody::Dict(entries)) => Some(entries),
            _ => None,
        }
    }

    /// The text of the text block `tag`.
    pub fn text(&self, tag: &str) -> Option<&str> {
        match self.block(tag).map(|block| &block.body) {
            Some(BlockBody::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// The items of the array block `tag`.
    pub fn list(&self, tag: &str) -> Option<&[String]> {
        match self.block(tag).map(|block| &block.body) {
            Some(BlockBody::List(items)) => Some(items),
            _ => None,
        }
    }

    /// The value of the enabled entry `key` in dictionary block `tag`.
    pub fn value(&self, tag: &str, key: &str) -> Option<&str> {
        self.dict(tag)?.iter().find(|entry| entry.enabled && entry.key == key).map(|entry| entry.value.as_str())
    }
}

/// Tags whose content is free text rather than `key: value` lines.
fn is_text_tag(tag: &str) -> bool {
    match tag {
        "body" | "tests" | "docs" => true,
        "body:form-urlencoded" | "body:multipart-form" | "body:file" => false,
        _ => tag.starts_with("body:") || tag.starts_with("script:"),
    }
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

enum Opener<'a> {
    Brace(&'a str),
    Bracket(&'a str),
    InlineList(&'a str, &'a str),
}

fn opener(trimmed: &str) -> Option<Opener<'_>> {
    if let Some(head) = trimmed.strip_suffix('{') {
        let head = head.trim();
        return valid_tag(head).then_some(Opener::Brace(head));
    }
    if let Some(head) = trimmed.strip_suffix('[') {
        let head = head.trim();
        return valid_tag(head).then_some(Opener::Bracket(head));
    }
    if let Some(inner) = trimmed.strip_suffix(']')
        && let Some(open) = inner.find('[')
    {
        let head = inner.get(..open)?.trim();
        let items = inner.get(open + 1..)?;
        return valid_tag(head).then_some(Opener::InlineList(head, items));
    }
    None
}

fn split_items(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_string)
}

fn unclosed(tag: &str) -> SpecError {
    SpecError::Invalid { what: "Bru file", detail: format!("block `{tag}` is never closed") }
}

/// Parse one `.bru` file.
pub fn parse(text: &str) -> Result<BruDoc, SpecError> {
    if text.len() > MAX_FILE_BYTES {
        return Err(SpecError::TooLarge { what: "Bru file", size: text.len(), cap: MAX_FILE_BYTES });
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut doc = BruDoc::default();
    let mut at = 0usize;
    while let Some(line) = lines.get(at) {
        at += 1;
        let trimmed = line.trim();
        let Some(found) = opener(trimmed) else { continue };
        if doc.blocks.len() >= MAX_BLOCKS {
            return Err(SpecError::Invalid { what: "Bru file", detail: format!("more than {MAX_BLOCKS} blocks") });
        }
        let block = match found {
            Opener::InlineList(tag, items) => Block { tag: tag.to_string(), body: BlockBody::List(split_items(items).collect()) },
            Opener::Bracket(tag) => {
                let mut items = Vec::new();
                loop {
                    let Some(next) = lines.get(at) else { return Err(unclosed(tag)) };
                    at += 1;
                    let next = next.trim();
                    if next == "]" {
                        break;
                    }
                    items.extend(split_items(next));
                }
                Block { tag: tag.to_string(), body: BlockBody::List(items) }
            }
            Opener::Brace(tag) if is_text_tag(tag) => {
                let mut body: Vec<&str> = Vec::new();
                loop {
                    let Some(next) = lines.get(at) else { return Err(unclosed(tag)) };
                    at += 1;
                    if next.trim_end() == "}" {
                        break;
                    }
                    body.push(next.strip_prefix("  ").unwrap_or(next));
                }
                Block { tag: tag.to_string(), body: BlockBody::Text(body.join("\n")) }
            }
            Opener::Brace(tag) => {
                let mut entries = Vec::new();
                let mut description: Option<String> = None;
                loop {
                    let Some(next) = lines.get(at) else { return Err(unclosed(tag)) };
                    at += 1;
                    if next.trim_end() == "}" {
                        break;
                    }
                    let next = next.trim();
                    if next.is_empty() {
                        continue;
                    }
                    if let Some(annotation) = next.strip_prefix('@') {
                        let args = annotation.split_once('(').map(|(name, rest)| (name.trim(), rest));
                        if let Some((name, rest)) = args
                            && let Some(after) = rest.trim_start().strip_prefix("'''")
                        {
                            let content = read_triple(after, &lines, &mut at);
                            if name == "description" {
                                description = Some(content);
                            }
                        } else if let Some((name, rest)) = args
                            && name == "description"
                        {
                            let value = rest.trim_end().trim_end_matches(')').trim().trim_matches(['"', '\'']);
                            description = Some(value.to_string());
                        }
                        continue;
                    }
                    if let Some(mut entry) = dict_entry(next, &lines, &mut at) {
                        entry.description = description.take();
                        entries.push(entry);
                    }
                }
                Block { tag: tag.to_string(), body: BlockBody::Dict(entries) }
            }
        };
        doc.blocks.push(block);
    }
    Ok(doc)
}

/// Text after an opening `'''`: the rest of the line, or more lines up to the closing `'''`.
fn read_triple(first: &str, lines: &[&str], at: &mut usize) -> String {
    if let Some((content, _)) = first.split_once("'''") {
        return content.to_string();
    }
    let mut parts = vec![first.to_string()];
    while let Some(next) = lines.get(*at) {
        *at += 1;
        if let Some((content, _)) = next.split_once("'''") {
            parts.push(content.to_string());
            break;
        }
        parts.push((*next).to_string());
    }
    let first_blank = parts.first().is_some_and(|p| p.trim().is_empty());
    let skip = usize::from(first_blank);
    let body: Vec<&str> = parts.iter().skip(skip).map(|line| line.strip_prefix("    ").unwrap_or(line.as_str())).collect();
    let joined = body.join("\n");
    joined.trim_end_matches([' ', '\t', '\n']).to_string()
}

fn dict_entry(line: &str, lines: &[&str], at: &mut usize) -> Option<DictEntry> {
    let (enabled, rest) = match line.strip_prefix('~') {
        Some(rest) => (false, rest.trim_start()),
        None => (true, line),
    };
    let (key, value) = split_key_value(rest)?;
    let key = unquote(key.trim());
    let mut value = value.trim().to_string();
    if let Some(after) = value.strip_prefix("'''") {
        value = read_triple(after, lines, at);
    } else if value == "[" {
        let mut items = Vec::new();
        while let Some(next) = lines.get(*at) {
            *at += 1;
            let next = next.trim();
            if next == "]" {
                break;
            }
            items.extend(split_items(next));
        }
        value = items.join(", ");
    }
    Some(DictEntry { key, value, enabled, description: None })
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    if let Some(quoted) = line.strip_prefix('"')
        && let Some(end) = quoted.find('"')
    {
        let after = quoted.get(end + 1..)?.trim_start();
        if let Some(value) = after.strip_prefix(':') {
            return Some((line.get(..end + 2)?, value));
        }
    }
    if let Some(index) = line.find(": ") {
        return Some((line.get(..index)?, line.get(index + 1..)?));
    }
    if let Some(key) = line.strip_suffix(':') {
        return Some((key, ""));
    }
    let index = line.find(':')?;
    Some((line.get(..index)?, line.get(index + 1..)?))
}

fn unquote(key: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = key.strip_prefix(quote).and_then(|rest| rest.strip_suffix(quote)) {
            return inner.to_string();
        }
    }
    key.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_text_and_list_blocks() {
        let doc =
            parse("meta {\n  name: A\n  ~seq: 3\n  empty:\n}\n\nbody:json {\n  {\n    \"a\": 1\n  }\n}\n\nvars:secret [\n  x,\n  ~y\n]\n").unwrap();
        let meta = doc.dict("meta").unwrap();
        assert_eq!(meta.len(), 3);
        assert!(!meta[1].enabled);
        assert_eq!(meta[2].value, "");
        assert_eq!(doc.text("body:json"), Some("{\n  \"a\": 1\n}"));
        assert_eq!(doc.list("vars:secret"), Some(&["x".to_string(), "~y".to_string()][..]));
    }

    #[test]
    fn unclosed_block_is_an_error() {
        assert!(parse("meta {\n  name: A\n").is_err());
    }
}
