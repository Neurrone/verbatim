//! A text reader of the declarations the state checks count
//! (`xtask/src/shared_state.rs`, `xtask/src/ledger.rs`): the fields of each
//! struct and enum variant, and each `static`, of a file's product code.
//!
//! It reads text, outside comments and string literals, with every item
//! marked `#[cfg(test)]` blanked, and finds an item by a line that starts,
//! after its visibility, with `struct`, `enum` or `union`. A field's type
//! is its text with runs of white space made one space. Block comments and
//! a type a macro writes are not read.

use std::collections::BTreeMap;

use crate::waits::{cfg_test_marks, item_end, strip_comments, strip_strings};

/// One field of a struct or of an enum's variant, or one `static`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declaration {
    /// The struct, `Enum::Variant`, or `static` for a static.
    pub owner: String,
    /// The field's name, its index in a tuple, or the static's name.
    pub name: String,
    /// The declared type, its white space made single spaces.
    pub ty: String,
    /// The line it starts on, counted from 1.
    pub line: usize,
}

impl Declaration {
    /// How a report names it: `Owner.field`, or `static NAME`.
    pub fn key(&self) -> String {
        if self.owner == "static" {
            format!("static {}", self.name)
        } else {
            format!("{}.{}", self.owner, self.name)
        }
    }
}

/// A struct or an enum, with its fields, a variant's as `Enum::Variant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub name: String,
    pub fields: Vec<Declaration>,
}

/// `source`'s lines without comments and string contents, each item marked
/// `#[cfg(test)]` blank.
pub(crate) fn product_code(source: &str) -> Vec<String> {
    let mut code: Vec<String> = source
        .lines()
        .map(|line| strip_strings(&strip_comments(line)))
        .collect();
    let marked = cfg_test_marks(&code);
    for (line, marked) in code.iter_mut().zip(marked) {
        if marked {
            line.clear();
        }
    }
    code
}

/// Whether `character` can be part of an identifier.
fn in_word(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// The identifiers of `text`.
pub(crate) fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !in_word(character))
        .filter(|word| !word.is_empty())
}

/// `text` without a leading visibility: `pub`, `pub(crate)`, `pub(super)`
/// or `pub(in path)`.
fn strip_visibility(text: &str) -> &str {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("pub") else {
        return trimmed;
    };
    if rest.trim_start().starts_with('(')
        && let Some(close) = rest.find(')')
    {
        return rest[close + 1..].trim_start();
    }
    if rest.starts_with(char::is_whitespace) {
        return rest.trim_start();
    }
    trimmed
}

/// `text` without its leading attributes, `#[...]`, and the number of
/// bytes removed.
fn strip_attributes(text: &str) -> (&str, usize) {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with("#[") {
            return (trimmed, text.len() - trimmed.len());
        }
        match closing(trimmed, 1) {
            Some(close) => rest = &trimmed[close + 1..],
            None => return ("", text.len()),
        }
    }
}

/// The index of the bracket closing the one at `open` in `text`, counting
/// `(`, `[` and `{` alike.
fn closing(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (at, character) in text.char_indices().skip_while(|(at, _)| *at < open) {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` split at the commas outside any bracket or generic list, each
/// part with its byte offset; a `>` of `->` closes nothing.
fn split_top(text: &str) -> Vec<(usize, &str)> {
    let mut parts = Vec::new();
    let (mut depth, mut from) = (0_i32, 0);
    let mut previous = ' ';
    for (at, character) in text.char_indices() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if previous != '-' => depth -= 1,
            ',' if depth == 0 => {
                parts.push((from, &text[from..at]));
                from = at + 1;
            }
            _ => {}
        }
        previous = character;
    }
    parts.push((from, &text[from..]));
    parts
        .into_iter()
        .filter(|(_, part)| !part.trim().is_empty())
        .collect()
}

/// `text`'s white space made single spaces.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The fields of a body, `inner`, between braces (`named`) or parentheses,
/// owned by `owner`, at byte `offset` of `joined`, whose first line is
/// `first_line`.
fn fields(
    owner: &str,
    inner: &str,
    named: bool,
    offset: usize,
    joined: &str,
    first_line: usize,
) -> Vec<Declaration> {
    let line_at = |at: usize| first_line + joined[..at].matches('\n').count();
    let mut found = Vec::new();
    for (index, (at, part)) in split_top(inner).into_iter().enumerate() {
        let (rest, skipped) = strip_attributes(part);
        let start = offset + at + skipped;
        let rest = strip_visibility(rest);
        if rest.is_empty() {
            continue;
        }
        if named {
            let name: String = rest.chars().take_while(|c| in_word(*c)).collect();
            let after = rest[name.len()..].trim_start();
            if name.is_empty() || !after.starts_with(':') || after.starts_with("::") {
                continue;
            }
            found.push(Declaration {
                owner: owner.to_owned(),
                name,
                ty: normalize(&after[1..]),
                line: line_at(start),
            });
        } else {
            found.push(Declaration {
                owner: owner.to_owned(),
                name: index.to_string(),
                ty: normalize(rest),
                line: line_at(start),
            });
        }
    }
    found
}

/// The structs and enums of `code` ([`product_code`]), each with its
/// fields and its variants' fields.
pub(crate) fn items(code: &[String]) -> Vec<Item> {
    let mut found = Vec::new();
    for start in 0..code.len() {
        let (line, _) = strip_attributes(&code[start]);
        let line = strip_visibility(line);
        let Some((kind, rest)) = ["struct ", "enum ", "union "]
            .iter()
            .find_map(|kind| line.strip_prefix(kind).map(|rest| (*kind, rest)))
        else {
            continue;
        };
        let name: String = rest.chars().take_while(|c| in_word(*c)).collect();
        if name.is_empty() {
            continue;
        }
        let end = item_end(code, start);
        let joined = code[start..end].join("\n");
        let Some(after_name) = joined.find(&format!("{kind}{name}")) else {
            continue;
        };
        let mut at = after_name + kind.len() + name.len();
        let mut angle = 0_i32;
        let mut previous = ' ';
        let mut open = None;
        for (offset, character) in joined[at..].char_indices() {
            match character {
                '<' => angle += 1,
                '>' if previous != '-' => angle -= 1,
                '{' | '(' if angle == 0 => {
                    open = Some((at + offset, character));
                    break;
                }
                ';' if angle == 0 => break,
                _ => {}
            }
            previous = character;
        }
        let mut item = Item {
            name: name.clone(),
            fields: Vec::new(),
        };
        if let Some((open, bracket)) = open
            && let Some(close) = closing(&joined, open)
        {
            at = open + 1;
            let inner = &joined[at..close];
            if kind == "enum " {
                item.fields = variants(&name, inner, at, &joined, start + 1);
            } else {
                item.fields = fields(&name, inner, bracket == '{', at, &joined, start + 1);
            }
        }
        found.push(item);
    }
    found
}

/// The fields of the variants of the enum `name`, whose body is `inner` at
/// byte `offset` of `joined`.
fn variants(
    name: &str,
    inner: &str,
    offset: usize,
    joined: &str,
    first_line: usize,
) -> Vec<Declaration> {
    let mut found = Vec::new();
    for (at, part) in split_top(inner) {
        let (rest, skipped) = strip_attributes(part);
        let variant: String = rest.chars().take_while(|c| in_word(*c)).collect();
        let after = rest[variant.len()..].trim_start();
        let Some(bracket) = after.chars().next().filter(|c| *c == '{' || *c == '(') else {
            continue;
        };
        let open = offset + at + skipped + (rest.len() - after.len());
        let Some(close) = closing(joined, open) else {
            continue;
        };
        found.extend(fields(
            &format!("{name}::{variant}"),
            &joined[open + 1..close],
            bracket == '{',
            open + 1,
            joined,
            first_line,
        ));
    }
    found
}

/// The `static`s of `code` ([`product_code`]), a `thread_local!`'s among
/// them; a lifetime, `'static`, is not one.
pub(crate) fn statics(code: &[String]) -> Vec<Declaration> {
    let mut found = Vec::new();
    for (index, line) in code.iter().enumerate() {
        for (at, _) in line.match_indices("static ") {
            let before = line[..at].chars().next_back();
            if before.is_some_and(|c| in_word(c) || c == '\'') {
                continue;
            }
            let rest = line[at + "static ".len()..].trim_start();
            let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
            let name: String = rest.chars().take_while(|c| in_word(*c)).collect();
            let after = rest[name.len()..].trim_start();
            if name.is_empty() || !after.starts_with(':') {
                continue;
            }
            let mut text = after[1..].to_owned();
            let mut next = index + 1;
            while !text.contains(['=', ';']) && next < code.len() && next < index + 20 {
                text.push(' ');
                text.push_str(&code[next]);
                next += 1;
            }
            let ty = text.split(['=', ';']).next().unwrap_or_default();
            found.push(Declaration {
                owner: "static".to_owned(),
                name,
                ty: normalize(ty),
                line: index + 1,
            });
        }
    }
    found
}

/// Every `.rs` file of the crate in `crates/<name>` under `root`, by its
/// path from `root` with `/` between names, with its text: the `src`
/// folder's, but the test modules in files of their own
/// ([`crate::coverage::test_module_files`]).
pub(crate) fn crate_sources(
    root: &std::path::Path,
    name: &str,
) -> Result<BTreeMap<String, String>, String> {
    let source = root.join("crates").join(name).join("src");
    let mut files = Vec::new();
    if source.is_dir() {
        crate::waits::collect_rust_files(&source, &mut files)?;
    }
    let mut sources = BTreeMap::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|error| format!("{}: {error}", file.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let text = std::fs::read_to_string(&file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        sources.insert(relative, text);
    }
    let mut tests = crate::coverage::test_module_files(&sources);
    tests.extend(test_path_files(&sources));
    sources.retain(|path, _| {
        !tests
            .iter()
            .any(|test| path == test || (test.ends_with('/') && path.starts_with(test.as_str())))
    });
    Ok(sources)
}

/// The files of the test modules `sources` declare with a path of their
/// own, `#[cfg(test)]`, then `#[path = "file.rs"]`, then `mod name;`, each
/// relative to the declaring file's folder.
fn test_path_files(sources: &BTreeMap<String, String>) -> Vec<String> {
    let mut found = Vec::new();
    for (path, source) in sources {
        let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
        let lines: Vec<&str> = source.lines().map(str::trim).collect();
        for (index, line) in lines.iter().enumerate() {
            if *line != "#[cfg(test)]" {
                continue;
            }
            let file = lines[index + 1..]
                .iter()
                .take_while(|line| line.starts_with("#["))
                .find_map(|line| {
                    line.strip_prefix("#[path = \"")
                        .and_then(|rest| rest.strip_suffix("\"]"))
                });
            if let Some(file) = file {
                found.push(format!("{folder}/{file}"));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(source: &str) -> Vec<String> {
        product_code(source)
    }

    #[test]
    fn a_structs_named_and_tuple_fields_are_read_with_their_types_and_lines() {
        let source = "/// A record.\n#[derive(Debug)]\npub(crate) struct Record<T: Clone> {\n    /// Its name.\n    pub name: String,\n    #[expect(dead_code)]\n    map: BTreeMap<u8,\n        Vec<T>>,\n    call: Box<dyn Fn(u8) -> u8>,\n}\nstruct Pair(pub u8, Arc<Mutex<u8>>);\nstruct Unit;\n";
        let found = items(&code(source));
        let fields: Vec<(String, String, usize)> = found
            .iter()
            .flat_map(|item| &item.fields)
            .map(|field| (field.key(), field.ty.clone(), field.line))
            .collect();
        assert_eq!(
            fields,
            [
                ("Record.name".to_owned(), "String".to_owned(), 5),
                (
                    "Record.map".to_owned(),
                    "BTreeMap<u8, Vec<T>>".to_owned(),
                    7
                ),
                (
                    "Record.call".to_owned(),
                    "Box<dyn Fn(u8) -> u8>".to_owned(),
                    9
                ),
                ("Pair.0".to_owned(), "u8".to_owned(), 11),
                ("Pair.1".to_owned(), "Arc<Mutex<u8>>".to_owned(), 11),
            ]
        );
        assert_eq!(
            found
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["Record", "Pair", "Unit"]
        );
    }

    #[test]
    fn an_enums_variant_fields_are_read_and_its_unit_variants_are_not() {
        let source = "enum Stage {\n    Idle,\n    #[default]\n    Waiting { since: Instant, asked: Cell<bool> },\n    Done(Reply),\n    Code = 3,\n}\n";
        let fields: Vec<String> = items(&code(source))
            .into_iter()
            .flat_map(|item| item.fields)
            .map(|field| format!("{}: {}", field.key(), field.ty))
            .collect();
        assert_eq!(
            fields,
            [
                "Stage::Waiting.since: Instant",
                "Stage::Waiting.asked: Cell<bool>",
                "Stage::Done.0: Reply"
            ]
        );
    }

    #[test]
    fn a_test_module_in_a_file_named_by_its_path_attribute_is_found() {
        let sources = BTreeMap::from([(
            "crates/a/src/keys/caret.rs".to_owned(),
            "fn a() {}\n#[cfg(test)]\n#[path = \"caret_tests.rs\"]\nmod tests;\n".to_owned(),
        )]);
        assert_eq!(
            test_path_files(&sources),
            ["crates/a/src/keys/caret_tests.rs"]
        );
    }

    #[test]
    fn statics_and_thread_locals_are_read_and_lifetimes_and_tests_are_not() {
        let source = "static COUNT: AtomicU64 = AtomicU64::new(0);\npub static mut RAW: u8 = 0;\nthread_local! {\n    static SEEN: RefCell<Vec<u8>>\n        = RefCell::new(Vec::new());\n}\nfn a(x: &'static str) -> impl Fn() + 'static {}\n#[cfg(test)]\nmod tests {\n    static HELD: Mutex<u8> = Mutex::new(0);\n}\n";
        let found: Vec<String> = statics(&code(source))
            .into_iter()
            .map(|field| format!("{} {}: {}", field.line, field.key(), field.ty))
            .collect();
        assert_eq!(
            found,
            [
                "1 static COUNT: AtomicU64",
                "2 static RAW: u8",
                "4 static SEEN: RefCell<Vec<u8>>"
            ]
        );
    }
}
