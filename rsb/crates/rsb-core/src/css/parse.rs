//! The style sheet parser: comments, rules, at-rules, declarations.

use std::sync::Arc;

use super::select::{Selector, parse_selector};
use super::value::{Ctx, FamilyName, Longhand, components, parse_property, split_top, unsupported_property};
use super::{PROPERTIES, PropertyId, PropertyInfo, Stylesheet, property_by_name};
use crate::diag::{DiagKind, Diagnostic, SourcePos};
use crate::element::ElementKind;

/// A parsed declaration.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Decl {
    pub target: DeclTarget,
    pub pos: Option<SourcePos>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DeclTarget {
    /// Parsed when the sheet was read.
    Ready(Vec<Longhand>),
    /// The value holds `var()`: parsed after substitution.
    Deferred { prop: PropertyId, raw: Arc<str>, inline: bool },
    /// `--name: value`.
    Custom { name: Arc<str>, raw: Arc<str> },
}

#[derive(Clone, Debug)]
pub(crate) struct Rule {
    pub sel: Selector,
    pub spec: (u32, u32, u32),
    /// `Some(true)` = only in dark, `Some(false)` = only in light.
    pub media: Option<bool>,
    pub decls: Arc<Vec<Decl>>,
}

pub(crate) fn property_info(id: PropertyId) -> &'static PropertyInfo {
    PROPERTIES.iter().find(|p| p.id == id).expect("every property id is in the table")
}

/// True when `value` uses `var(` outside a string.
pub(crate) fn has_var(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'v' if lower[i..].starts_with("var(")
                && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'-') =>
            {
                return true;
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Parses one declaration (stylesheet or inline). Problems become
/// diagnostics at `pos`; None = the declaration is dropped.
pub(crate) fn parse_decl(
    name: &str,
    value: &str,
    pos: Option<SourcePos>,
    ctx: Ctx,
    diags: &mut Vec<Diagnostic>,
    families: Option<&mut Vec<(String, SourcePos)>>,
) -> Option<Decl> {
    let warn = |diags: &mut Vec<Diagnostic>, msg: String| {
        diags.push(Diagnostic::warning(DiagKind::Style, pos.clone(), msg));
    };
    if let Some(custom) = name.strip_prefix("--") {
        if custom.is_empty() || !custom.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            warn(diags, format!("bad custom property name '{name}': use letters, digits, '-' and '_'"));
            return None;
        }
        if value.trim().is_empty() {
            warn(diags, format!("custom property '{name}' has no value"));
            return None;
        }
        return Some(Decl { target: DeclTarget::Custom { name: Arc::from(name), raw: Arc::from(value.trim()) }, pos });
    }
    let lower = name.to_ascii_lowercase();
    let Some(info) = property_by_name(&lower) else {
        if let Some(why) = unsupported_property(&lower) {
            warn(diags, format!("'{name}' {why}"));
        } else {
            warn(
                diags,
                format!("unknown property '{name}'{}", crate::suggest::hint(&lower, PROPERTIES.iter().map(|p| p.name))),
            );
        }
        return None;
    };
    if has_var(value) {
        return Some(Decl {
            target: DeclTarget::Deferred { prop: info.id, raw: Arc::from(value.trim()), inline: ctx.inline },
            pos,
        });
    }
    match parse_property(info, value, ctx) {
        Ok(longhands) => {
            if info.id == PropertyId::FontFamily
                && let (Some(out), Some(p)) = (families, pos.as_ref())
            {
                for l in &longhands {
                    if let Longhand::FontFamily(list) = l {
                        for f in list {
                            if let FamilyName::Named(n) = f {
                                out.push((n.to_string(), p.clone()));
                            }
                        }
                    }
                }
            }
            Some(Decl { target: DeclTarget::Ready(longhands), pos })
        }
        Err(reason) => {
            warn(diags, format!("bad value for '{}': {reason}", info.name));
            None
        }
    }
}

/// Replaces comments with spaces (newlines kept) so offsets and lines stay.
fn strip_comments(text: &str, file: &str, li: &LineIndex, diags: &mut Vec<Diagnostic>) -> String {
    let mut bytes = text.as_bytes().to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != q && bytes[i] != b'\n' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                let end = text[i + 2..].find("*/").map(|e| i + 2 + e + 2);
                let stop = end.unwrap_or(bytes.len());
                if end.is_none() {
                    let (l, c) = li.line_col(text, start);
                    diags.push(Diagnostic::error(
                        DiagKind::Style,
                        Some(SourcePos::new(file, l, c)),
                        "unterminated comment; close it with */",
                    ));
                }
                for b in &mut bytes[start..stop] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = stop;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    String::from_utf8(bytes).unwrap_or_default()
}

pub(crate) struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(text.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
        LineIndex { starts }
    }

    /// 1-based line and column (chars) of a byte offset.
    pub fn line_col(&self, text: &str, off: usize) -> (u32, u32) {
        let off = off.min(text.len());
        let line = self.starts.partition_point(|&s| s <= off) - 1;
        let start = self.starts[line];
        let mut col_end = off;
        while !text.is_char_boundary(col_end) {
            col_end -= 1;
        }
        (line as u32 + 1, text[start..col_end].chars().count() as u32 + 1)
    }
}

struct Parser<'a> {
    src: &'a str,
    orig: &'a str,
    file: &'a str,
    li: LineIndex,
    diags: Vec<Diagnostic>,
    sheet: Stylesheet,
    seen_rule: bool,
}

impl Parser<'_> {
    fn pos(&self, off: usize) -> SourcePos {
        let (l, c) = self.li.line_col(self.orig, off);
        SourcePos::new(self.file, l, c)
    }

    fn warn(&mut self, off: usize, msg: impl Into<String>) {
        let p = self.pos(off);
        self.diags.push(Diagnostic::warning(DiagKind::Style, Some(p), msg));
    }

    fn error(&mut self, off: usize, msg: impl Into<String>) {
        let p = self.pos(off);
        self.diags.push(Diagnostic::error(DiagKind::Style, Some(p), msg));
    }

    fn skip_ws(&self, mut i: usize, end: usize) -> usize {
        let b = self.src.as_bytes();
        while i < end && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    }

    /// First `target` byte at brace/paren depth 0 outside strings, in `from..end`.
    fn find_top(&self, from: usize, end: usize, targets: &[u8]) -> Option<usize> {
        let b = self.src.as_bytes();
        let (mut i, mut depth) = (from, 0i32);
        while i < end {
            let c = b[i];
            if depth == 0 && targets.contains(&c) {
                return Some(i);
            }
            match c {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                q @ (b'"' | b'\'') => {
                    i += 1;
                    while i < end && b[i] != q && b[i] != b'\n' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Index of the `}` matching the `{` at `open`.
    fn block_end(&self, open: usize, end: usize) -> Option<usize> {
        let b = self.src.as_bytes();
        let (mut i, mut depth) = (open, 0i32);
        while i < end {
            match b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                q @ (b'"' | b'\'') => {
                    i += 1;
                    while i < end && b[i] != q && b[i] != b'\n' {
                        if b[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn parse_rules(&mut self, start: usize, end: usize, media: Option<bool>) {
        let mut i = start;
        loop {
            i = self.skip_ws(i, end);
            if i >= end {
                return;
            }
            match self.src.as_bytes()[i] {
                b'@' => i = self.at_rule(i, end, media),
                b'}' => {
                    self.error(i, "unexpected '}'");
                    i += 1;
                }
                _ => i = self.rule(i, end, media),
            }
        }
    }

    fn rule(&mut self, start: usize, end: usize, media: Option<bool>) -> usize {
        self.seen_rule = true;
        let Some(open) = self.find_top(start, end, b"{};") else {
            self.error(start, "expected '{' after the selector");
            return end;
        };
        if self.src.as_bytes()[open] != b'{' {
            self.error(open, "expected '{' after the selector");
            return open + 1;
        }
        let (body_end, next) = match self.block_end(open, end) {
            Some(c) => (c, c + 1),
            None => {
                let p = self.pos(open);
                self.diags.push(Diagnostic::error(DiagKind::Style, Some(p), "this '{' is never closed; add a '}'"));
                (end, end)
            }
        };
        let mut sels: Vec<Selector> = Vec::new();
        let prelude = &self.src[start..open];
        for part in split_top(prelude, b',') {
            let off = part.as_ptr() as usize - self.src.as_ptr() as usize;
            let lead = part.len() - part.trim_start().len();
            let text = part.trim();
            if text.is_empty() {
                self.warn(off, "expected a selector");
                continue;
            }
            match parse_selector(text) {
                Ok(s) => sels.push(s),
                Err((o, msg)) => self.warn(off + lead + o, msg),
            }
        }
        let decls = self.block(open + 1, body_end);
        let decls = Arc::new(decls);
        for sel in sels {
            if self.has_backdrop(&decls)
                && let Some(k) = sel.subject_kind()
                && k != ElementKind::Bar
            {
                let p = decls.iter().find_map(backdrop_pos).flatten().or_else(|| Some(self.pos(open)));
                self.diags.push(Diagnostic::warning(DiagKind::Style, p, "backdrop only works on bar"));
            }
            let spec = sel.specificity();
            self.sheet.rules.push(Rule { sel, spec, media, decls: decls.clone() });
        }
        next
    }

    fn has_backdrop(&self, decls: &[Decl]) -> bool {
        decls.iter().any(|d| backdrop_pos(d).is_some())
    }

    fn block(&mut self, start: usize, end: usize) -> Vec<Decl> {
        let mut out = Vec::new();
        let mut side_color: Option<(crate::color::Color, SourcePos)> = None;
        let body = &self.src[start..end];
        let mut pieces: Vec<(usize, &str)> = Vec::new();
        {
            // Split at ';' outside parens and strings; braces inside a piece
            // are nested rules, reported below.
            let mut offset = 0;
            for piece in split_top(body, b';') {
                pieces.push((start + offset, piece));
                offset += piece.len() + 1;
            }
        }
        for (off, piece) in pieces {
            let lead = piece.len() - piece.trim_start().len();
            let text = piece.trim();
            if text.is_empty() {
                continue;
            }
            let at = off + lead;
            if text.contains('{') || text.contains('}') {
                self.warn(at, "nested rules are not supported; write each rule on its own");
                continue;
            }
            let Some(colon) = self.find_top(at, at + text.len(), b":") else {
                self.warn(at, format!("expected 'property: value', found '{}'", short(text)));
                continue;
            };
            let name = self.src[at..colon].trim();
            let vstart_raw = &self.src[colon + 1..at + text.len()];
            let vlead = vstart_raw.len() - vstart_raw.trim_start().len();
            let mut value = vstart_raw.trim();
            let vat = colon + 1 + vlead;
            if name.is_empty() {
                self.warn(at, "expected a property name before ':'");
                continue;
            }
            let lower_value = value.to_ascii_lowercase();
            if let Some(stripped) = lower_value.strip_suffix("important")
                && let Some(before) = stripped.trim_end().strip_suffix('!')
            {
                self.warn(
                    vat + before.len(),
                    "'!important' is not supported; raise the selector's specificity instead",
                );
                value = value[..before.len()].trim_end();
            }
            if value.is_empty() {
                self.warn(vat, format!("'{name}' has no value"));
                continue;
            }
            let mut diags = Vec::new();
            let mut fams = Vec::new();
            // Diagnostics for a declaration point at the property name.
            let name_pos = self.pos(at);
            let d = parse_decl(name, value, Some(name_pos.clone()), Ctx { inline: false }, &mut diags, Some(&mut fams));
            self.diags.extend(diags);
            self.sheet.families.extend(fams);
            if let Some(d) = d {
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "border-top" | "border-right" | "border-bottom" | "border-left"
                ) && let DeclTarget::Ready(ls) = &d.target
                {
                    for l in ls {
                        if let Longhand::BorderColor(super::value::ColorVal::Color(c)) = l {
                            match &side_color {
                                Some((prev, _)) if prev != c => {
                                    self.diags.push(Diagnostic::warning(
                                        DiagKind::Style,
                                        Some(name_pos.clone()),
                                        "a box has one border colour; the last one set wins",
                                    ));
                                }
                                _ => {}
                            }
                            side_color = Some((*c, name_pos.clone()));
                        }
                    }
                }
                out.push(d);
            }
        }
        out
    }

    fn at_rule(&mut self, start: usize, end: usize, media: Option<bool>) -> usize {
        let b = self.src.as_bytes();
        let mut i = start + 1;
        while i < end && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        let name = self.src[start + 1..i].to_ascii_lowercase();
        let stop = self.find_top(i, end, b"{;");
        let prelude_end = stop.unwrap_or(end);
        let prelude = self.src[i..prelude_end].trim().to_string();
        let is_block = stop.is_some_and(|s| b[s] == b'{');
        let next = match stop {
            None => end,
            Some(s) if !is_block => s + 1,
            Some(s) => self.block_end(s, end).map_or(end, |c| c + 1),
        };
        match name.as_str() {
            "import" => self.import(start, &prelude, is_block, media),
            "media" => {
                if media.is_some() {
                    self.warn(start, "@media cannot be nested");
                } else if !is_block {
                    self.warn(start, "@media needs a '{ ... }' block");
                } else {
                    let open = stop.unwrap_or(end);
                    let close = self.block_end(open, end).unwrap_or(end);
                    match parse_media(&prelude) {
                        Some(dark) => {
                            self.seen_rule = true;
                            self.parse_rules(open + 1, close, Some(dark));
                        }
                        None => self.warn(
                            start,
                            "only '@media (prefers-color-scheme: dark)' and '(prefers-color-scheme: light)' are supported",
                        ),
                    }
                }
            }
            "keyframes" | "-webkit-keyframes" => {
                self.warn(start, "animations come in a later version; @keyframes is not supported")
            }
            "define-color" => self.warn(start, "@define-color is not supported; use a custom property: --name: value"),
            _ => self.warn(start, format!("unsupported at-rule '@{name}'")),
        }
        next
    }

    fn import(&mut self, start: usize, prelude: &str, is_block: bool, media: Option<bool>) {
        if is_block || media.is_some() {
            self.warn(start, "@import must be a plain '@import \"file.css\";' before every rule");
            return;
        }
        if self.seen_rule {
            self.warn(start, "@import must come before every rule");
            return;
        }
        let comps = match components(prelude) {
            Ok(c) => c,
            Err(e) => {
                self.warn(start, format!("bad @import: {e}"));
                return;
            }
        };
        let path = match comps.first() {
            Some(super::value::Comp::Str(s)) => s.clone(),
            Some(super::value::Comp::Func(_, n, inner)) if n == "url" => {
                inner.trim().trim_matches(['"', '\'']).to_string()
            }
            _ => {
                self.warn(start, "expected @import \"file.css\";");
                return;
            }
        };
        if comps.len() > 1 {
            self.warn(start, "media conditions on @import are not supported");
            return;
        }
        let bad = path.is_empty()
            || path.starts_with('/')
            || path.starts_with('\\')
            || path.contains(':')
            || path.split(['/', '\\']).any(|s| s == "..")
            || !path.to_ascii_lowercase().ends_with(".css");
        if bad {
            self.warn(start, "an @import path must be a .css file inside the config folder, like \"colors.css\"");
            return;
        }
        let p = self.pos(start);
        self.sheet.imports.push((path.replace('\\', "/"), p));
    }
}

fn backdrop_pos(d: &Decl) -> Option<Option<SourcePos>> {
    let is = match &d.target {
        DeclTarget::Ready(ls) => ls.iter().any(|l| matches!(l, Longhand::Backdrop(_))),
        DeclTarget::Deferred { prop, .. } => *prop == PropertyId::Backdrop,
        DeclTarget::Custom { .. } => false,
    };
    is.then(|| d.pos.clone())
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(24).collect();
    if t.len() < s.len() { format!("{t}...") } else { t }
}

/// `(prefers-color-scheme: dark)` -> Some(true).
fn parse_media(prelude: &str) -> Option<bool> {
    let inner = prelude.trim().strip_prefix('(')?.strip_suffix(')')?;
    let (k, v) = inner.split_once(':')?;
    if !k.trim().eq_ignore_ascii_case("prefers-color-scheme") {
        return None;
    }
    match v.trim().to_ascii_lowercase().as_str() {
        "dark" => Some(true),
        "light" => Some(false),
        _ => None,
    }
}

pub(crate) fn parse_sheet(text: &str, file: &str) -> (Stylesheet, Vec<Diagnostic>) {
    let li = LineIndex::new(text);
    let mut diags = Vec::new();
    let clean = strip_comments(text, file, &li, &mut diags);
    let mut p = Parser { src: &clean, orig: text, file, li, diags, sheet: Stylesheet::default(), seen_rule: false };
    p.parse_rules(0, clean.len(), None);
    (p.sheet, p.diags)
}
