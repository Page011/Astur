//! The colours block at the end of `style.css`: parsed, rendered and
//! replaced as one unit, and checked against the rest of the file.
//!
//! The block is marked by two comment lines. Inside it there is one rule per
//! colour scheme holding the six accent variables. A block that holds
//! anything else is somebody's edit and is refused, never rewritten.

use super::Splice;
use super::text::{ends_with_break, eol_of, line_end_incl, line_number, line_start};
use crate::contract::{AccentSet, ConfigFile, ErrorCode, SettingsError};
use crate::css::Stylesheet;
use crate::element::ElementKind;

/// The two comment lines that bound the block, compared trimmed.
pub const START: &str = "/* Colours set by rsb settings. Edit them here or on the settings page. */";
pub const END: &str = "/* End of colours set by rsb settings. */";

/// The six variables, in the order they are written.
pub const VARS: [&str; 6] = ["accent", "accent-fg", "accent-hover", "accent-press", "accent-text", "accent-line"];

/// A rule's specificity.
type Spec = (u32, u32, u32);

/// The values of one scheme's rule, in `VARS` order.
pub(crate) type Values = [String; 6];

/// A colours block found in a file.
#[derive(Clone, Debug)]
pub(crate) struct Block {
    /// Offset of the first byte of the start line.
    pub start: usize,
    /// Offset just after the end line's break.
    pub end: usize,
    pub dark: Values,
    pub light: Values,
}

fn broken(text: &str, at: usize, why: &str) -> SettingsError {
    let place = format!("style.css:{}", line_number(text, at));
    SettingsError::new(ErrorCode::ColorsBlockBroken, format!("{place}: {why}"))
        .with_file(ConfigFile::StyleCss)
        .with_at(place)
        .with_next("put back the end line, or delete the block")
}

fn edited(text: &str, at: usize) -> SettingsError {
    let place = format!("style.css:{}", line_number(text, at));
    SettingsError::new(
        ErrorCode::ColorsBlockEdited,
        format!("{place}: the colours block holds a rule RSB did not write"),
    )
    .with_file(ConfigFile::StyleCss)
    .with_at(place)
    .with_next("move your own rules below the end line")
}

/// The block of `text`, or None when there is none. Marker trouble is
/// `colors_block_broken`, other content `colors_block_edited`.
pub(crate) fn find_block(text: &str) -> Result<Option<Block>, SettingsError> {
    let Some((start, end_line)) = scan_markers(text)? else { return Ok(None) };
    let end = line_end_incl(text, end_line);
    let inner_from = line_end_incl(text, start);
    let inner = &text[inner_from..end_line];
    match parse_inner(inner) {
        Some((dark, light)) => Ok(Some(Block { start, end, dark, light })),
        None => Err(edited(text, odd_line(text, inner_from, end_line))),
    }
}

/// The offsets of the start line and of the end line of the block, whatever
/// is between them. None when there are no markers at all.
pub(crate) fn scan_markers(text: &str) -> Result<Option<(usize, usize)>, SettingsError> {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        let next = line_end_incl(text, at);
        match text[at..next].trim() {
            START => starts.push(at),
            END => ends.push(at),
            _ => {}
        }
        at = next;
    }
    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => return Ok(None),
        ([], [e, ..]) => return Err(broken(text, *e, "the colours block has an end line but no start line")),
        ([_, second, ..], _) => return Err(broken(text, *second, "the colours block has two start lines")),
        ([s], []) => return Err(broken(text, *s, "the colours block has no end line")),
        ([s], [e]) if e < s => {
            return Err(broken(text, *e, "the end line of the colours block comes before its start line"));
        }
        ([_], [_, second, ..]) => return Err(broken(text, *second, "the colours block has two end lines")),
        _ => {}
    }
    Ok(Some((starts[0], ends[0])))
}

/// The first inner line that is not one of the shapes RSB writes.
fn odd_line(text: &str, from: usize, to: usize) -> usize {
    let mut at = from;
    while at < to {
        let next = line_end_incl(text, at);
        let t = text[at..next].trim();
        let shape = t.is_empty()
            || t == "}"
            || (t.starts_with("@media (prefers-color-scheme:") && t.ends_with('{'))
            || (t.starts_with(":root {") && t.ends_with('}') && t.matches(';').count() == VARS.len());
        if !shape {
            return at;
        }
        at = next;
    }
    from
}

/// The two rules of a block, with every blank taken out.
fn parse_inner(inner: &str) -> Option<(Values, Values)> {
    let mut s: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out: Vec<Values> = Vec::new();
    for scheme in ["dark", "light"] {
        s = s.strip_prefix(&format!("@media(prefers-color-scheme:{scheme}){{:root{{"))?.to_string();
        let mut values: Vec<String> = Vec::new();
        for var in VARS {
            s = s.strip_prefix(&format!("--{var}:"))?.to_string();
            // `#rrggbb`, or the short `#rgb` a person types by hand, kept as the long form.
            let end = s.find(';')?;
            let hex = &s[..end];
            let digits = hex.strip_prefix('#')?;
            if !matches!(digits.len(), 3 | 6) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let long: String =
                if digits.len() == 3 { digits.chars().flat_map(|c| [c, c]).collect() } else { digits.to_string() };
            values.push(format!("#{}", long.to_ascii_lowercase()));
            s = s[end + 1..].to_string();
        }
        s = s.strip_prefix("}}")?.to_string();
        out.push(values.try_into().ok()?);
    }
    if !s.is_empty() {
        return None;
    }
    let light = out.pop()?;
    let dark = out.pop()?;
    Some((dark, light))
}

/// The block's text for `set`, every line ending in `eol`.
pub(crate) fn render(set: &AccentSet, eol: &str) -> String {
    let rule = |scheme: &str, t: &crate::contract::AccentTokens| {
        format!(
            "@media (prefers-color-scheme: {scheme}) {{{eol}    :root {{ --accent: {v}; --accent-fg: {fg}; --accent-hover: {hover}; --accent-press: {press}; --accent-text: {text}; --accent-line: {line}; }}{eol}}}{eol}",
            v = set.value,
            fg = t.fg,
            hover = t.hover,
            press = t.press,
            text = t.text,
            line = t.line,
        )
    };
    format!("{START}{eol}{}{}{END}{eol}", rule("dark", &set.schemes.dark), rule("light", &set.schemes.light))
}

/// The values a block holds, for comparing it with what was meant.
pub(crate) fn values_of(set: &AccentSet) -> (Values, Values) {
    let row = |t: &crate::contract::AccentTokens| -> Values {
        [set.value.clone(), t.fg.clone(), t.hover.clone(), t.press.clone(), t.text.clone(), t.line.clone()]
    };
    (row(&set.schemes.dark), row(&set.schemes.light))
}

/// The splices that put `block_text` into `text`: over the existing block, or
/// appended after a blank line.
pub(crate) fn put_block(text: &str, existing: Option<&Block>, block_text: &str) -> Splice {
    match existing {
        Some(b) => Splice { start: b.start, end: b.end, text: block_text.to_string() },
        None => {
            let eol = eol_of(text);
            let lead = if ends_with_break(text) { "" } else { eol };
            let tail = text.trim_end_matches([' ', '\t']);
            let blank_before =
                tail.is_empty() || tail.ends_with("\n\n") || tail.ends_with("\r\n\r\n") || tail.ends_with("\n\r\n");
            let blank = if blank_before { "" } else { eol };
            Splice { start: text.len(), end: text.len(), text: format!("{lead}{blank}{block_text}") }
        }
    }
}

/// The splice that takes the block out, with the blank line before it when
/// that line separates it from other text.
pub(crate) fn remove_block(text: &str, b: &Block) -> Splice {
    let mut start = b.start;
    if start > 0 {
        let prev = line_start(text, start - 1);
        let before_prev_blank = prev == 0 || text[line_start(text, prev - 1)..prev].trim().is_empty();
        if text[prev..start].trim().is_empty() && !before_prev_blank {
            start = prev;
        }
    }
    Splice { start, end: b.end, text: String::new() }
}

/// The dark scheme's accent of the block in `text`, when there is a readable one.
pub(crate) fn accent_of(text: &str) -> Option<String> {
    find_block(text).ok().flatten().map(|b| b.dark[0].clone())
}

/// One declaration of an accent token that bears on the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// Where it is, `file:line:column`.
    pub at: String,
    /// The token it sets, without the dashes (`accent-text`).
    pub token: String,
}

/// What else says something about the accent tokens: later rules that win
/// over the block, and earlier declarations (in `style.css` above the block,
/// or in a file it imports) that the block now beats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conflicts {
    pub shadowed: Vec<Conflict>,
    pub replaced: Vec<Conflict>,
}

/// The first and last line (1-based) of the colours block of `text`,
/// whatever its content, when both markers are there once and in order.
pub fn block_lines(text: &str) -> Option<(u32, u32)> {
    let (start, end_line) = scan_markers(text).ok().flatten()?;
    Some((line_number(text, start) as u32, line_number(text, end_line) as u32))
}

/// The conflicts of the colours block of `style_text`, read from `sheet`: the
/// whole chain (imported files first, then `style.css`; the built-in sheets
/// may be in it and are ignored). None when the text has no block.
pub fn accent_conflicts(sheet: &Stylesheet, style_text: &str) -> Option<Conflicts> {
    let (first_line, last_line) = block_lines(style_text)?;
    Some(conflicts(sheet, first_line, last_line))
}

fn conflicts(sheet: &Stylesheet, first_line: u32, last_line: u32) -> Conflicts {
    // Specificity of the block's own rule (`:root`).
    let mut block_spec: Option<Spec> = None;
    let mut found: Vec<(Spec, Conflict, bool)> = Vec::new();
    for rule in &sheet.rules {
        let targets_bar = rule.sel.parts.len() == 1 && rule.sel.parts[0].1.kind == Some(ElementKind::Bar);
        if !targets_bar {
            continue;
        }
        for decl in rule.decls.iter() {
            let (crate::css::parse::DeclTarget::Custom { name, .. }, Some(pos)) = (&decl.target, &decl.pos) else {
                continue;
            };
            let Some(token) = name.strip_prefix("--").filter(|n| VARS.contains(n)) else { continue };
            let own = &*pos.file == "style.css";
            if own && pos.line >= first_line && pos.line <= last_line {
                block_spec = Some(rule.spec);
            } else if own || !(&*pos.file == crate::css::BASE_SHEET || &*pos.file == crate::css::PRESET_SHEET) {
                // A file that style.css imports comes before the whole of style.css.
                let earlier = !own || pos.line < first_line;
                let at = format!("{}:{}:{}", pos.file, pos.line, pos.col);
                found.push((rule.spec, Conflict { at, token: token.to_string() }, earlier));
            }
        }
    }
    let mut out = Conflicts::default();
    if let Some(spec) = block_spec {
        for (rule_spec, conflict, earlier) in found {
            let wins = rule_spec > spec || (!earlier && rule_spec == spec);
            let list = if wins {
                &mut out.shadowed
            } else if earlier {
                &mut out.replaced
            } else {
                continue;
            };
            if !list.iter().any(|c| c.at == conflict.at) {
                list.push(conflict);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{AccentSchemes, AccentTokens};

    fn set() -> AccentSet {
        let t = |fg: &str| AccentTokens {
            fg: fg.into(),
            fg_ratio: 10.0,
            hover: "#e8ae22".into(),
            press: "#eab73b".into(),
            text: "#e5a50a".into(),
            line: "#e5a50a".into(),
            moved: 0,
        };
        AccentSet { value: "#e5a50a".into(), schemes: AccentSchemes { dark: t("#000000"), light: t("#111111") } }
    }

    #[test]
    fn a_rendered_block_reads_back_to_the_same_values() {
        for eol in ["\n", "\r\n"] {
            let block = render(&set(), eol);
            let text = format!("a {{ color: red }}{eol}{eol}{block}");
            let found = find_block(&text).unwrap().unwrap();
            let (dark, light) = values_of(&set());
            assert_eq!((found.dark, found.light), (dark, light));
            assert_eq!(&text[found.start..found.end], block);
            assert_eq!(accent_of(&text).as_deref(), Some("#e5a50a"));
        }
        assert!(find_block("bar { color: red }\n").unwrap().is_none());
    }

    #[test]
    fn marker_trouble_names_the_line() {
        let block = render(&set(), "\n");
        let no_end = block.replace(END, "");
        assert_eq!(find_block(&format!("x\n{no_end}")).unwrap_err().code, ErrorCode::ColorsBlockBroken);
        let two = format!("{block}{block}");
        let e = find_block(&two).unwrap_err();
        assert_eq!(e.code, ErrorCode::ColorsBlockBroken);
        assert!(e.message.starts_with("style.css:"), "{}", e.message);
        let swapped = format!("{END}\n{block}").replacen(END, "", 1);
        let _ = swapped;
        let only_end = format!("a {{}}\n{END}\n");
        assert_eq!(find_block(&only_end).unwrap_err().code, ErrorCode::ColorsBlockBroken);
    }

    #[test]
    fn a_short_colour_typed_by_hand_inside_the_block_is_read_as_the_long_one() {
        let by_hand = render(
            &set(),
            "
",
        )
        .replace("--accent-hover: #e8ae22;", "--accent-hover: #FA0;");
        let b = find_block(&by_hand).unwrap().unwrap();
        assert_eq!((b.dark[2].as_str(), b.light[2].as_str()), ("#ffaa00", "#ffaa00"));
    }

    #[test]
    fn another_rule_inside_the_block_is_an_edit() {
        let block = render(&set(), "\n").replace(END, &format!("clock {{ color: red }}\n{END}"));
        let e = find_block(&block).unwrap_err();
        assert_eq!(e.code, ErrorCode::ColorsBlockEdited);
        assert!(e.message.starts_with("style.css:"), "{}", e.message);
        let changed = render(&set(), "\n").replace("--accent-fg: #000000;", "--accent-fg: red;");
        assert_eq!(find_block(&changed).unwrap_err().code, ErrorCode::ColorsBlockEdited);
    }

    #[test]
    fn a_block_goes_in_after_a_blank_line_and_comes_out_with_it() {
        let block = render(&set(), "\n");
        let s = put_block("a {}", None, &block);
        let text = format!("a {{}}{}", s.text);
        assert!(text.starts_with("a {}\n\n/* Colours"), "{text}");
        let b = find_block(&text).unwrap().unwrap();
        let r = remove_block(&text, &b);
        assert_eq!(super::super::text::apply_splices(&text, &[r]), "a {}\n");
        let s = put_block("a {}\n\n", None, &block);
        assert!(s.text.starts_with("/* Colours"));
        let empty = put_block("", None, &block);
        assert_eq!(empty.text, block);
    }

    #[test]
    fn later_and_earlier_declarations_are_told_apart() {
        let block = render(&set(), "\n");
        let text = format!(
            ":root {{ --accent: #123456; }}\n\n{block}\nbar {{ --accent-fg: #fff; }}\nbar.main {{ --accent: red; }}\n"
        );
        let sheet = Stylesheet::parse(&text, "style.css").0;
        let c = accent_conflicts(&sheet, &text).unwrap();
        let places = |l: &[Conflict]| l.iter().map(|c| format!("{} {}", c.at, c.token)).collect::<Vec<_>>();
        assert_eq!(places(&c.replaced), vec!["style.css:1:9 accent".to_string()]);
        assert_eq!(places(&c.shadowed).len(), 2, "{:?}", c.shadowed);
        assert!(places(&c.shadowed).iter().any(|p| p.ends_with("accent-fg")), "the token is named");
        let clean = format!("clock {{ color: red }}\n\n{block}");
        let sheet = Stylesheet::parse(&clean, "style.css").0;
        let c = accent_conflicts(&sheet, &clean).unwrap();
        assert!(c.shadowed.is_empty() && c.replaced.is_empty());
        assert!(accent_conflicts(&sheet, "bar { color: red }\n").is_none(), "no block, nothing to say");
    }

    #[test]
    fn a_declaration_in_an_imported_file_is_an_earlier_one() {
        let block = render(&set(), "\n");
        let own = format!("@import \"colors.css\";\n\n{block}");
        let imported = ":root { --accent: #ff0000; }\nbar.main { --accent-text: #00ff00; }\n";
        let mut sheet = Stylesheet::parse(imported, "colors.css").0;
        sheet.append(Stylesheet::parse(&own, "style.css").0);
        let c = accent_conflicts(&sheet, &own).unwrap();
        assert_eq!(c.replaced.iter().map(|c| c.at.as_str()).collect::<Vec<_>>(), vec!["colors.css:1:9"]);
        assert_eq!(c.shadowed.iter().map(|c| c.at.as_str()).collect::<Vec<_>>(), vec!["colors.css:2:12"]);
        // The built-in sheets are not the user's declarations.
        let mut with_preset = Stylesheet::parse(":root { --accent: #123456; }\n", crate::css::PRESET_SHEET).0;
        with_preset.append(Stylesheet::parse(&own, "style.css").0);
        assert_eq!(accent_conflicts(&with_preset, &own).unwrap(), Conflicts::default());
    }

    #[test]
    fn the_lines_of_a_block_are_found_whatever_is_inside() {
        let block = render(&set(), "\n");
        let text = format!("a {{}}\n\n{block}b {{}}\n");
        assert_eq!(block_lines(&text), Some((3, 10)));
        assert_eq!(block_lines("a {}\n"), None);
        assert_eq!(block_lines(&format!("{START}\n")), None, "no end line");
        assert_eq!(block_lines(&format!("{END}\n{START}\n")), None, "end before start");
        assert_eq!(block_lines(&format!("{START}\r\nx\r\n{END}\r\n")), Some((1, 3)), "CRLF files and an edited block");
    }
}
