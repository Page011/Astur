//! Edits of `bar.toml` as byte splices. The file is parsed with the spans the
//! TOML parser reports; a change is a value span replaced, a line inserted or
//! removed, or one item of an array added or taken out. Nothing else is
//! touched, so comments, order and line endings stay as the person left them.
//!
//! Every function here takes the text and what `parse` found in it and
//! returns splices against that text. Text written into the file is built by
//! the caller from validated values.

use std::ops::Range;

use toml::de::{DeTable, DeValue};

use super::Splice;
use super::text::{
    ends_with_break, eol_of, indent_at, line_end_excl, line_end_incl, line_number, line_start, only_blanks,
};
use crate::config::plain_toml_message;
use crate::contract::{ConfigFile, ErrorCode, SettingsError};
use crate::css::parse::LineIndex;

/// The kind of value an entry holds, as far as an edit needs to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Str,
    Int,
    Float,
    Bool,
    Array,
    Table,
    Other,
}

/// One item of an array.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub start: usize,
    pub end: usize,
    /// The text of a string item; None for any other value.
    pub text: Option<String>,
}

/// One `key = value` line (or lines, for an array that spans several).
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub key: String,
    pub key_start: usize,
    pub val_start: usize,
    pub val_end: usize,
    pub kind: Kind,
    pub items: Vec<Item>,
}

/// One `[[bar]]` table.
#[derive(Clone, Debug)]
pub(crate) struct BarTable {
    /// Offset of the `[[`.
    pub header: usize,
    pub entries: Vec<Entry>,
    /// A key written with dots, or a sub-table, lives inside this table.
    pub has_subtable: bool,
}

impl BarTable {
    pub(crate) fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }
}

/// How `bar` is written in the file.
#[derive(Clone, Debug)]
pub(crate) enum Bars {
    /// No `bar` key.
    Absent,
    /// `bar = []`.
    Empty,
    /// `[[bar]]` tables, in file order.
    Tables(Vec<BarTable>),
    /// Anything else: offset of the line to name in the refusal.
    Unsupported { at: usize, why: &'static str },
}

/// What the parse of a `bar.toml` found.
#[derive(Clone, Debug)]
pub(crate) struct Info {
    /// Root-level `key = value` entries (tables are not listed).
    pub root: Vec<Entry>,
    pub bars: Bars,
    /// Offset of the first byte of every live table header line.
    pub headers: Vec<usize>,
}

impl Info {
    pub(crate) fn root_entry(&self, key: &str) -> Option<&Entry> {
        self.root.iter().find(|e| e.key == key)
    }

    pub(crate) fn tables(&self) -> &[BarTable] {
        match &self.bars {
            Bars::Tables(t) => t,
            _ => &[],
        }
    }

    /// The first live header after `pos`, or the end of the text.
    pub(crate) fn next_header(&self, pos: usize, text_len: usize) -> usize {
        self.headers.iter().copied().find(|h| *h > pos).unwrap_or(text_len)
    }
}

fn kind_of(v: &DeValue) -> Kind {
    match v {
        DeValue::String(_) => Kind::Str,
        DeValue::Integer(_) => Kind::Int,
        DeValue::Float(_) => Kind::Float,
        DeValue::Boolean(_) => Kind::Bool,
        DeValue::Array(_) => Kind::Array,
        DeValue::Table(_) => Kind::Table,
        DeValue::Datetime(_) => Kind::Other,
    }
}

fn entry_of(key: &str, k: Range<usize>, v: &toml::Spanned<DeValue<'_>>) -> Entry {
    let items = match v.get_ref() {
        DeValue::Array(a) => a
            .iter()
            .map(|e| Item {
                start: e.span().start,
                end: e.span().end,
                text: match e.get_ref() {
                    DeValue::String(s) => Some(s.to_string()),
                    _ => None,
                },
            })
            .collect(),
        _ => Vec::new(),
    };
    Entry {
        key: key.to_string(),
        key_start: k.start,
        val_start: v.span().start,
        val_end: v.span().end,
        kind: kind_of(v.get_ref()),
        items,
    }
}

/// True when the source at `at` is the `[[` of a table header.
fn is_header_at(text: &str, at: usize) -> bool {
    text[at..].starts_with("[[")
}

/// The ranges of every value that is not a table: a line that starts inside
/// one is a continuation, never a header.
fn value_ranges(text: &str, table: &DeTable<'_>, out: &mut Vec<Range<usize>>) {
    for (_, v) in table.iter() {
        match v.get_ref() {
            DeValue::Table(t) => value_ranges(text, t, out),
            DeValue::Array(a)
                if a.iter().next().is_some_and(|e| {
                    matches!(e.get_ref(), DeValue::Table(_)) && is_header_at(text, e.span().start)
                }) =>
            {
                for e in a.iter() {
                    if let DeValue::Table(t) = e.get_ref() {
                        value_ranges(text, t, out);
                    }
                }
            }
            _ => out.push(v.span()),
        }
    }
}

/// Parses `text` (no byte order mark). A syntax error is `file_broken` with
/// the line and column.
pub(crate) fn parse(text: &str) -> Result<Info, SettingsError> {
    let table = DeTable::parse(text).map_err(|e| {
        let li = LineIndex::new(text);
        let (line, col) = e.span().map_or((0, 0), |r| li.line_col(text, r.start));
        let msg = plain_toml_message(e.message().trim_end());
        let at = if line > 0 { format!("bar.toml:{line}:{col}") } else { "bar.toml".to_string() };
        SettingsError::new(ErrorCode::FileBroken, format!("{at}: {msg}"))
            .with_file(ConfigFile::BarToml)
            .with_at(at)
            .with_next("fix that line, or run: rsb settings restore bar.toml (your broken copy is kept)")
    })?;
    let root_table = table.get_ref();
    let mut root = Vec::new();
    let mut bars = Bars::Absent;
    for (k, v) in root_table.iter() {
        let key = k.get_ref().as_ref();
        match (key, v.get_ref()) {
            ("bar", DeValue::Array(a)) => {
                let first = a.iter().next();
                bars = match first {
                    None => Bars::Empty,
                    Some(e) if is_header_at(text, e.span().start) => {
                        let mut tables = Vec::new();
                        for e in a.iter() {
                            if let DeValue::Table(t) = e.get_ref() {
                                let mut entries: Vec<Entry> = Vec::new();
                                let mut sub = false;
                                for (ek, ev) in t.iter() {
                                    if matches!(ev.get_ref(), DeValue::Table(_)) {
                                        sub = true;
                                    }
                                    entries.push(entry_of(ek.get_ref().as_ref(), ek.span(), ev));
                                }
                                entries.sort_by_key(|e| e.key_start);
                                tables.push(BarTable { header: e.span().start, entries, has_subtable: sub });
                            }
                        }
                        tables.sort_by_key(|t| t.header);
                        Bars::Tables(tables)
                    }
                    Some(e) => Bars::Unsupported {
                        at: e.span().start,
                        why: "bars written as inline tables cannot be changed here",
                    },
                };
            }
            ("bar", DeValue::Table(_)) => {
                bars = Bars::Unsupported { at: v.span().start, why: "bars written as [bar] cannot be changed here" };
            }
            ("bar", _) => {
                bars = Bars::Unsupported { at: v.span().start, why: "'bar' is not a list of [[bar]] tables" };
            }
            (_, DeValue::Table(_)) => {}
            _ => root.push(entry_of(key, k.span(), v)),
        }
    }
    root.sort_by_key(|e| e.key_start);
    let mut ranges = Vec::new();
    value_ranges(text, root_table, &mut ranges);
    let mut headers = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        let end = line_end_incl(text, at);
        let line = &text[at..end];
        let trimmed = line.trim_start_matches([' ', '\t']);
        let inside = ranges.iter().any(|r| at > r.start && at < r.end);
        if !inside && trimmed.starts_with('[') {
            headers.push(at);
        }
        at = end;
    }
    Ok(Info { root, bars, headers })
}

/// A refusal that names the line: `bar.toml:12: <why>`.
pub(crate) fn unsupported(text: &str, at: usize, why: &str) -> SettingsError {
    let place = format!("bar.toml:{}", line_number(text, at));
    SettingsError::new(ErrorCode::UnsupportedLayout, format!("{place}: {why}"))
        .with_file(ConfigFile::BarToml)
        .with_at(place)
        .with_next("change that line by hand")
}

// ---------------------------------------------------------------- examples

/// A commented `# key = value` line of the template.
pub(crate) struct Example {
    /// Offset of the `#`.
    pub hash: usize,
    /// Offset of the key.
    pub key_start: usize,
    pub val_start: usize,
    pub val_end: usize,
}

/// The end of the value that starts at `i` of `line`: a string, a one-line
/// array, or a bare word. None when it does not end on this line.
fn scan_value(line: &str, i: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut j = i;
    match b.get(i)? {
        b'"' | b'\'' => {
            let quote = b[i];
            j += 1;
            while j < b.len() && b[j] != quote {
                if quote == b'"' && b[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            (j < b.len()).then_some(j + 1)
        }
        b'[' => {
            let mut depth = 0i32;
            while j < b.len() {
                match b[j] {
                    q @ (b'"' | b'\'') => {
                        j += 1;
                        while j < b.len() && b[j] != q {
                            if q == b'"' && b[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                    }
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            while j < b.len() && !matches!(b[j], b' ' | b'\t' | b'#' | b'\r' | b'\n') {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

/// The first commented example of `key` between `from` and `to`.
pub(crate) fn find_example(text: &str, from: usize, to: usize, key: &str) -> Option<Example> {
    let to = to.min(text.len());
    let mut at = from;
    while at < to {
        let end = line_end_incl(text, at);
        if end <= at {
            break;
        }
        if let Some(ex) = match_example(&text[at..end], at, key) {
            return Some(ex);
        }
        at = end;
    }
    None
}

fn match_example(line: &str, base: usize, key: &str) -> Option<Example> {
    let blanks = |s: &str| s.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    let hash = blanks(line);
    if !line[hash..].starts_with('#') {
        return None;
    }
    let mut i = hash + 1;
    i += blanks(&line[i..]);
    let key_start = i;
    let rest = line[i..].strip_prefix(key)?;
    if !rest.starts_with([' ', '\t', '=']) {
        return None;
    }
    i += key.len();
    i += blanks(&line[i..]);
    if !line[i..].starts_with('=') {
        return None;
    }
    i += 1;
    i += blanks(&line[i..]);
    let val_end = scan_value(line, i)?;
    let tail = line[val_end..].trim();
    if !(tail.is_empty() || tail.starts_with('#')) {
        return None;
    }
    Some(Example { hash: base + hash, key_start: base + key_start, val_start: base + i, val_end: base + val_end })
}

/// The splices that turn an example into the live line with `value`.
fn live_example(ex: &Example, value: &str) -> Vec<Splice> {
    vec![
        Splice { start: ex.hash, end: ex.key_start, text: String::new() },
        Splice { start: ex.val_start, end: ex.val_end, text: value.to_string() },
    ]
}

// ---------------------------------------------------------------- one key

/// The result of setting a key: splices, and the 1-based line of an example
/// that was turned live.
pub(crate) struct KeyEdit {
    pub splices: Vec<Splice>,
    pub example_line: Option<usize>,
}

/// The new line for a key that has none: after `after_pos`'s line, with
/// `indent`. A text that does not end with a line break gets one first.
fn new_line(text: &str, at: usize, indent: &str, line: &str) -> Splice {
    let eol = eol_of(text);
    let lead = if at >= text.len() && !ends_with_break(text) { eol } else { "" };
    Splice { start: at, end: at, text: format!("{lead}{indent}{line}{eol}") }
}

/// Sets `key` in the root table to `value` (rendered TOML).
pub(crate) fn set_root(text: &str, info: &Info, key: &str, value: &str) -> Result<KeyEdit, SettingsError> {
    if let Some(e) = info.root_entry(key) {
        return replace_value(text, e, key, value);
    }
    let region_end = info.headers.first().copied().unwrap_or(text.len());
    if let Some(ex) = find_example(text, 0, region_end, key) {
        let line = line_number(text, ex.hash);
        return Ok(KeyEdit { splices: live_example(&ex, value), example_line: Some(line) });
    }
    let line = format!("{key} = {value}");
    let at = info.root_entry("config_version").map_or(0, |e| line_end_incl(text, e.val_end));
    Ok(KeyEdit { splices: vec![new_line(text, at, "", &line)], example_line: None })
}

/// Sets `key` in a `[[bar]]` table.
pub(crate) fn set_bar_key(
    text: &str,
    info: &Info,
    table: &BarTable,
    key: &str,
    value: &str,
) -> Result<KeyEdit, SettingsError> {
    if table.has_subtable {
        return Err(unsupported(text, table.header, "dotted keys under [[bar]] cannot be changed here"));
    }
    if let Some(e) = table.entry(key) {
        return replace_value(text, e, key, value);
    }
    let from = line_end_incl(text, table.header);
    let to = info.next_header(table.header, text.len());
    if let Some(ex) = find_example(text, from, to, key) {
        let line = line_number(text, ex.hash);
        return Ok(KeyEdit { splices: live_example(&ex, value), example_line: Some(line) });
    }
    let line = format!("{key} = {value}");
    let splice = match table.entries.last() {
        Some(last) => {
            let end = table.entries.iter().map(|e| e.val_end).max().unwrap_or(last.val_end);
            new_line(text, line_end_incl(text, end), indent_at(text, last.key_start), &line)
        }
        None => new_line(text, from, "", &line),
    };
    Ok(KeyEdit { splices: vec![splice], example_line: None })
}

/// The value of an existing entry replaced, when it is the kind the loader
/// reads for this key.
fn replace_value(text: &str, e: &Entry, key: &str, value: &str) -> Result<KeyEdit, SettingsError> {
    let want_str = value.starts_with('"');
    let ok = match e.kind {
        Kind::Str => want_str,
        Kind::Bool => value == "true" || value == "false",
        Kind::Int | Kind::Float => value.bytes().all(|b| b.is_ascii_digit()),
        Kind::Array => value.starts_with('['),
        _ => false,
    };
    if !ok {
        return Err(unsupported(
            text,
            e.key_start,
            &format!("'{key}' holds a value of another type; change it by hand"),
        ));
    }
    Ok(KeyEdit {
        splices: vec![Splice { start: e.val_start, end: e.val_end, text: value.to_string() }],
        example_line: None,
    })
}

/// Removes the whole line (or lines) of an entry: key, value, trailing
/// comment and line break. Comment lines around it stay.
pub(crate) fn unset_entry(text: &str, e: &Entry) -> Splice {
    Splice { start: line_start(text, e.key_start), end: line_end_incl(text, e.val_end), text: String::new() }
}

// ---------------------------------------------------------------- bars

/// Splices that give a file with no live `[[bar]]` its `count` empty tables.
/// Returns the splices and whether the template's own commented line was
/// turned live (so its commented keys now sit inside a live table).
pub(crate) fn add_bar_tables(text: &str, info: &Info, count: usize) -> (Vec<Splice>, bool) {
    // The template's `# [[bar]]` becomes the live header in place, unless a
    // root key was written below it (that key would end up inside the table).
    if count == 1 {
        let mut at = 0usize;
        while at < text.len() {
            let end = line_end_incl(text, at);
            let line = text[at..end].trim_end_matches(['\r', '\n']);
            if line == "# [[bar]]" && !info.root.iter().any(|e| e.key_start > at) {
                return (vec![Splice { start: at, end: at + 2, text: String::new() }], true);
            }
            at = end;
        }
    }
    let eol = eol_of(text);
    let headers = format!("[[bar]]{eol}").repeat(count);
    let root_end = info.root.iter().map(|e| line_end_incl(text, e.val_end)).max().unwrap_or(0);
    match info.headers.first() {
        Some(first) => {
            let at = (*first).max(root_end);
            (vec![Splice { start: at, end: at, text: format!("{headers}{eol}") }], false)
        }
        None => {
            let at = text.len().max(root_end);
            let lead = if ends_with_break(text) { "" } else { eol };
            let blank = if text.is_empty() { "" } else { eol };
            (vec![Splice { start: at, end: at, text: format!("{lead}{blank}{headers}") }], false)
        }
    }
}

// ---------------------------------------------------------------- arrays

/// The splices that take item `idx` out of the array of `e`.
pub(crate) fn array_remove(text: &str, e: &Entry, idx: usize) -> Splice {
    let it = &e.items[idx];
    let (mut start, mut end) = (it.start, it.end);
    let blanks = |from: usize| from + text[from..].bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    let after = blanks(end);
    if text[after..].starts_with(',') {
        end = blanks(after + 1);
    } else if idx > 0 {
        // The last item: the comma that joins it to the one before goes too,
        // when both are on one line.
        let prev_end = e.items[idx - 1].end;
        let between = &text[prev_end..start];
        if let Some(c) = between.find(',')
            && between.bytes().all(|b| matches!(b, b' ' | b'\t' | b','))
        {
            start = prev_end + c;
        }
    }
    // An item alone on its line takes the line with it.
    let (ls, le) = (line_start(text, start), line_end_incl(text, end));
    if text[ls..start].trim().is_empty() && text[end..line_end_excl(text, end)].trim().is_empty() {
        start = ls;
        end = le;
    }
    Splice { start, end, text: String::new() }
}

/// The splices that put `item` (a quoted token) into the array of `e` before
/// item `idx` (`idx == len` appends).
pub(crate) fn array_insert(text: &str, e: &Entry, idx: usize, item: &str) -> Vec<Splice> {
    let eol = eol_of(text);
    let n = e.items.len();
    if n == 0 {
        let at = e.val_start + 1;
        return vec![Splice { start: at, end: at, text: item.to_string() }];
    }
    if idx < n {
        let it = &e.items[idx];
        let ls = line_start(text, it.start);
        if only_blanks(text, ls, it.start) {
            let indent = &text[ls..it.start];
            return vec![Splice { start: ls, end: ls, text: format!("{indent}{item},{eol}") }];
        }
        return vec![Splice { start: it.start, end: it.start, text: format!("{item}, ") }];
    }
    let last = &e.items[n - 1];
    let blanks = |from: usize| from + text[from..].bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    let after = blanks(last.end);
    let comma = text[after..].starts_with(',').then_some(after);
    let ls = line_start(text, last.start);
    let rest_from = comma.map_or(after, |c| c + 1);
    let rest = text[rest_from..line_end_excl(text, rest_from)].trim();
    let own_line = only_blanks(text, ls, last.start) && (rest.is_empty() || rest.starts_with('#'));
    if own_line {
        let indent = &text[ls..last.start];
        let at = line_end_incl(text, last.end);
        let mut out = Vec::new();
        if comma.is_none() {
            out.push(Splice { start: last.end, end: last.end, text: ",".to_string() });
        }
        let tail = if comma.is_some() { "," } else { "" };
        out.push(Splice { start: at, end: at, text: format!("{indent}{item}{tail}{eol}") });
        return out;
    }
    match comma {
        Some(c) => vec![Splice { start: c + 1, end: c + 1, text: format!(" {item},") }],
        None => vec![Splice { start: last.end, end: last.end, text: format!(", {item}") }],
    }
}

#[cfg(test)]
mod tests {
    use super::super::text::apply_splices;
    use super::*;

    fn run(text: &str, f: impl Fn(&Entry) -> Vec<Splice>) -> String {
        let info = parse(text).unwrap();
        let e = info.tables()[0].entry("left").cloned().unwrap();
        apply_splices(text, &f(&e))
    }

    #[test]
    fn items_come_out_with_their_comma_and_leave_comments() {
        let t = "[[bar]]\nleft = [\"a\", \"b\", \"c\"]\n";
        assert_eq!(run(t, |e| vec![array_remove(t, e, 0)]), "[[bar]]\nleft = [\"b\", \"c\"]\n");
        assert_eq!(run(t, |e| vec![array_remove(t, e, 1)]), "[[bar]]\nleft = [\"a\", \"c\"]\n");
        assert_eq!(run(t, |e| vec![array_remove(t, e, 2)]), "[[bar]]\nleft = [\"a\", \"b\"]\n");
        let m = "[[bar]]\nleft = [\n  \"a\", # first\n  \"b\",\n  \"c\"\n]\n";
        assert_eq!(run(m, |e| vec![array_remove(m, e, 1)]), "[[bar]]\nleft = [\n  \"a\", # first\n  \"c\"\n]\n");
        assert_eq!(run(m, |e| vec![array_remove(m, e, 2)]), "[[bar]]\nleft = [\n  \"a\", # first\n  \"b\",\n]\n");
        assert_eq!(run(m, |e| vec![array_remove(m, e, 0)]), "[[bar]]\nleft = [\n  # first\n  \"b\",\n  \"c\"\n]\n");
        let one = "[[bar]]\nleft = [\"a\"]\n";
        assert_eq!(run(one, |e| vec![array_remove(one, e, 0)]), "[[bar]]\nleft = []\n");
    }

    #[test]
    fn items_go_in_in_the_style_of_the_array() {
        let t = "[[bar]]\nleft = [\"a\", \"b\"]\n";
        assert_eq!(run(t, |e| array_insert(t, e, 0, "\"x\"")), "[[bar]]\nleft = [\"x\", \"a\", \"b\"]\n");
        assert_eq!(run(t, |e| array_insert(t, e, 2, "\"x\"")), "[[bar]]\nleft = [\"a\", \"b\", \"x\"]\n");
        let empty = "[[bar]]\nleft = []\n";
        assert_eq!(run(empty, |e| array_insert(empty, e, 0, "\"x\"")), "[[bar]]\nleft = [\"x\"]\n");
        let m = "[[bar]]\nleft = [\n  \"a\", # first\n  \"b\"\n]\n";
        assert_eq!(
            run(m, |e| array_insert(m, e, 2, "\"x\"")),
            "[[bar]]\nleft = [\n  \"a\", # first\n  \"b\",\n  \"x\"\n]\n"
        );
        assert_eq!(
            run(m, |e| array_insert(m, e, 1, "\"x\"")),
            "[[bar]]\nleft = [\n  \"a\", # first\n  \"x\",\n  \"b\"\n]\n"
        );
        let tc = "[[bar]]\nleft = [\n  \"a\",\n  \"b\",\n]\n";
        assert_eq!(
            run(tc, |e| array_insert(tc, e, 2, "\"x\"")),
            "[[bar]]\nleft = [\n  \"a\",\n  \"b\",\n  \"x\",\n]\n"
        );
        let closing = "[[bar]]\nleft = [\n  \"a\",\n  \"b\"]\n";
        assert_eq!(
            run(closing, |e| array_insert(closing, e, 2, "\"x\"")),
            "[[bar]]\nleft = [\n  \"a\",\n  \"b\", \"x\"]\n"
        );
    }

    #[test]
    fn an_example_line_is_found_only_when_it_is_a_commented_assignment() {
        let t = "# height = 36                # logical pixels\n# height = whatever you like\n# heights = 3\n";
        let ex = find_example(t, 0, t.len(), "height").unwrap();
        assert_eq!(&t[ex.val_start..ex.val_end], "36");
        assert!(find_example(t, 0, t.len(), "heights").is_some());
        assert!(find_example("# height = whatever you like\n", 0, 29, "height").is_none());
        assert!(find_example("# the height = 3\n", 0, 17, "height").is_none());
        let a = "# right = [\"status\", \"clock\"]    # note\n";
        let ex = find_example(a, 0, a.len(), "right").unwrap();
        assert_eq!(&a[ex.val_start..ex.val_end], "[\"status\", \"clock\"]");
    }

    #[test]
    fn headers_inside_a_multi_line_value_are_not_headers() {
        let t = "[[bar]]\nleft = [\n  [\"not\"],\n]\n\n[widget.clock]\nformat = \"x\"\n";
        let info = parse(t).unwrap();
        assert_eq!(info.headers, vec![0, t.find("[widget").unwrap()]);
    }

    #[test]
    fn a_syntax_error_names_line_and_column() {
        let err = parse("a = 1\nheight = \n").unwrap_err();
        assert_eq!(err.code, ErrorCode::FileBroken);
        assert!(err.message.starts_with("bar.toml:2:"), "{}", err.message);
    }
}
