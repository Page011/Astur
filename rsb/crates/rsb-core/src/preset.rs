//! Preset metadata: the `[about]` table at the end of each preset's TOML.
//!
//! It names the preset for a gallery, lists the colours worth tuning and
//! lists every colour pair the preset draws, so contrast can be measured
//! without running the bar. Only the preset layer takes it; in a user's
//! `bar.toml` it is ignored with a warning.

use std::borrow::Cow;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::css::parse::LineIndex;
use crate::diag::{DiagKind, Diagnostic, SourcePos};

/// Longest title, description and tune label, in characters.
pub const MAX_TITLE: usize = 32;
pub const MAX_DESCRIPTION: usize = 100;
pub const MAX_LABEL: usize = 20;

/// A colour scheme a preset's CSS defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scheme {
    Dark,
    Light,
}

impl Scheme {
    pub const ALL: [Scheme; 2] = [Scheme::Dark, Scheme::Light];

    pub fn name(self) -> &'static str {
        match self {
            Scheme::Dark => "dark",
            Scheme::Light => "light",
        }
    }

    pub fn from_name(name: &str) -> Option<Scheme> {
        Scheme::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// A colour a page may offer a picker for: a custom property of the preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tune {
    /// The custom property without its `--`: `accent`.
    pub var: String,
    pub label: String,
}

/// The pairs a preset draws, as lines of the form `<fg> on <surface> ...`.
/// `a+b` in a surface is a stack of translucent layers, bottom first; names
/// are custom properties without `--`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContrastPairs {
    /// Text: 4.5:1.
    pub text: Vec<String>,
    /// Icons, large text, the off state: 3:1.
    pub icon: Vec<String>,
    /// Rings, outlines, progress: 3:1.
    pub line: Vec<String>,
    /// Colours that carry no text and are not measured.
    pub decorative: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct About {
    pub title: String,
    pub description: String,
    /// A non-empty subset of dark and light: the schemes the CSS defines.
    pub schemes: Vec<Scheme>,
    /// In page order; `accent` is always among them.
    pub tune: Vec<Tune>,
    pub contrast: ContrastPairs,
}

/// Most lines in one contrast list.
pub const MAX_PAIR_LINES: usize = 128;
/// Longest custom property name, in characters.
pub const MAX_NAME: usize = 40;

/// A custom property name as the pair lists and `tune` write it: no `--`.
pub fn is_var_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
        && !name.starts_with('-')
}

/// A surface as a pair line writes it: names joined by `+`, bottom layer
/// first (`bg+pill`). None when a name is not a custom property name.
pub fn surface_layers(surface: &str) -> Option<Vec<&str>> {
    let layers: Vec<&str> = surface.split('+').collect();
    layers.iter().all(|l| is_var_name(l)).then_some(layers)
}

/// `fg on surface surface ...` as (fg, surfaces). None when the line does not
/// have that shape or a name is not a custom property name.
pub fn split_pair_line(line: &str) -> Option<(&str, Vec<&str>)> {
    let mut words = line.split_whitespace();
    let fg = words.next()?;
    if words.next()? != "on" || !is_var_name(fg) {
        return None;
    }
    let surfaces: Vec<&str> = words.collect();
    (!surfaces.is_empty() && surfaces.iter().all(|s| surface_layers(s).is_some())).then_some((fg, surfaces))
}

struct Reader<'a> {
    text: &'a str,
    file: &'a str,
    li: LineIndex,
}

impl Reader<'_> {
    fn at(&self, offset: usize) -> SourcePos {
        let (line, col) = self.li.line_col(self.text, offset);
        SourcePos::new(self.file, line, col)
    }

    fn fail<T>(&self, offset: usize, message: impl Into<String>) -> Result<T, Diagnostic> {
        Err(Diagnostic::error(DiagKind::Config, Some(self.at(offset)), message))
    }

    fn string(&self, key: &str, v: &Spanned<DeValue>) -> Result<String, Diagnostic> {
        match v.get_ref() {
            DeValue::String(s) => Ok(s.to_string()),
            other => self.fail(v.span().start, format!("'{key}' must be a string, got {}", type_name(other))),
        }
    }

    fn short_line(&self, key: &str, v: &Spanned<DeValue>, max: usize) -> Result<String, Diagnostic> {
        let s = self.string(key, v)?;
        if s.trim().is_empty() {
            return self.fail(v.span().start, format!("'{key}' must not be empty"));
        }
        if s.contains(['\n', '\r']) {
            return self.fail(v.span().start, format!("'{key}' must be one line"));
        }
        let n = s.chars().count();
        if n > max {
            return self.fail(v.span().start, format!("'{key}' is {n} characters; the limit is {max}"));
        }
        Ok(s)
    }

    fn array<'v, 'i>(&self, key: &str, v: &'v Spanned<DeValue<'i>>) -> Result<&'v [Spanned<DeValue<'i>>], Diagnostic> {
        match v.get_ref() {
            DeValue::Array(a) => Ok(a.as_ref()),
            other => self.fail(v.span().start, format!("'{key}' must be a list, got {}", type_name(other))),
        }
    }

    fn unknown<T>(&self, table: &str, key: &Spanned<Cow<'_, str>>, known: &[&str]) -> Result<T, Diagnostic> {
        let hint = crate::suggest::hint(key.get_ref(), known.iter().copied());
        self.fail(key.span().start, format!("unknown key '{}' in [{table}]{hint}", key.get_ref()))
    }

    fn schemes(&self, v: &Spanned<DeValue>) -> Result<Vec<Scheme>, Diagnostic> {
        let mut out = Vec::new();
        for e in self.array("schemes", v)? {
            let name = self.string("schemes", e)?;
            let Some(s) = Scheme::from_name(&name) else {
                return self.fail(e.span().start, format!("'{name}' is not a scheme; use \"dark\" or \"light\""));
            };
            if out.contains(&s) {
                return self.fail(e.span().start, format!("scheme \"{name}\" is listed twice"));
            }
            out.push(s);
        }
        if out.is_empty() {
            return self.fail(v.span().start, "'schemes' must list at least one of \"dark\" and \"light\"");
        }
        Ok(out)
    }

    fn tune(&self, v: &Spanned<DeValue>) -> Result<Vec<Tune>, Diagnostic> {
        let mut out: Vec<Tune> = Vec::new();
        for e in self.array("tune", v)? {
            let DeValue::Table(t) = e.get_ref() else {
                return self.fail(
                    e.span().start,
                    "'tune' entries must be tables like { var = \"accent\", label = \"Accent\" }",
                );
            };
            let (mut var, mut label) = (None, None);
            for (k, val) in t.iter() {
                match k.get_ref().as_ref() {
                    "var" => var = Some((self.string("var", val)?, val.span().start)),
                    "label" => label = Some(self.short_line("label", val, MAX_LABEL)?),
                    _ => return self.unknown("tune entry", k, &["var", "label"]),
                }
            }
            let Some((var, at)) = var else {
                return self.fail(e.span().start, "a 'tune' entry needs a 'var'");
            };
            let Some(label) = label else {
                return self.fail(e.span().start, "a 'tune' entry needs a 'label'");
            };
            if !is_var_name(&var) {
                return self
                    .fail(at, format!("'{var}' is not a custom property name; write it without the leading --"));
            }
            if out.iter().any(|t| t.var == var) {
                return self.fail(at, format!("'{var}' is listed twice in 'tune'"));
            }
            out.push(Tune { var, label });
        }
        if !out.iter().any(|t| t.var == "accent") {
            return self
                .fail(v.span().start, "'tune' must include the accent: { var = \"accent\", label = \"Accent\" }");
        }
        Ok(out)
    }

    fn lines(&self, key: &str, v: &Spanned<DeValue>, pairs: bool) -> Result<Vec<String>, Diagnostic> {
        let items = self.array(key, v)?;
        if items.len() > MAX_PAIR_LINES {
            return self
                .fail(v.span().start, format!("'{key}' has {} entries; the limit is {MAX_PAIR_LINES}", items.len()));
        }
        let mut out = Vec::new();
        for e in items {
            let line = self.string(key, e)?;
            let ok = if pairs { split_pair_line(&line).is_some() } else { is_var_name(&line) };
            if !ok {
                let shape = if pairs {
                    "'<colour> on <surface> <surface>' with names written without --, layers joined by +"
                } else {
                    "a custom property name written without --"
                };
                return self.fail(e.span().start, format!("'{line}' in '{key}' is not {shape}"));
            }
            out.push(line);
        }
        Ok(out)
    }

    fn contrast(&self, v: &Spanned<DeValue>) -> Result<ContrastPairs, Diagnostic> {
        let DeValue::Table(t) = v.get_ref() else {
            return self.fail(v.span().start, format!("'contrast' must be a table, got {}", type_name(v.get_ref())));
        };
        let mut out = ContrastPairs::default();
        for (k, val) in t.iter() {
            match k.get_ref().as_ref() {
                "text" => out.text = self.lines("text", val, true)?,
                "icon" => out.icon = self.lines("icon", val, true)?,
                "line" => out.line = self.lines("line", val, true)?,
                "decorative" => out.decorative = self.lines("decorative", val, false)?,
                _ => return self.unknown("about.contrast", k, &["text", "icon", "line", "decorative"]),
            }
        }
        Ok(out)
    }

    fn about(&self, t: &DeTable, at: usize) -> Result<About, Diagnostic> {
        let (mut title, mut description, mut schemes, mut tune, mut contrast) = (None, None, None, None, None);
        for (k, v) in t.iter() {
            match k.get_ref().as_ref() {
                "title" => title = Some(self.short_line("title", v, MAX_TITLE)?),
                "description" => description = Some(self.short_line("description", v, MAX_DESCRIPTION)?),
                "schemes" => schemes = Some(self.schemes(v)?),
                "tune" => tune = Some(self.tune(v)?),
                "contrast" => contrast = Some(self.contrast(v)?),
                _ => return self.unknown("about", k, &["title", "description", "schemes", "tune", "contrast"]),
            }
        }
        let need = |name: &str| -> Diagnostic {
            Diagnostic::error(DiagKind::Config, Some(self.at(at)), format!("[about] needs '{name}'"))
        };
        Ok(About {
            title: title.ok_or_else(|| need("title"))?,
            description: description.ok_or_else(|| need("description"))?,
            schemes: schemes.ok_or_else(|| need("schemes"))?,
            tune: tune.ok_or_else(|| need("tune"))?,
            contrast: contrast.unwrap_or_default(),
        })
    }
}

fn type_name(v: &DeValue) -> &'static str {
    match v {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "a list",
        DeValue::Table(_) => "a table",
    }
}

/// Reads the `[about]` table of a preset's TOML. `file` names the text in
/// the error (`presets/glass.toml`). A missing or malformed table is an
/// error with file:line:col.
pub fn parse_about(text: &str, file: &str) -> Result<About, Diagnostic> {
    let li = LineIndex::new(text);
    let parsed = match DeTable::parse(text) {
        Ok(p) => p,
        Err(e) => {
            let pos = e.span().map(|r| {
                let (line, col) = li.line_col(text, r.start);
                SourcePos::new(file, line, col)
            });
            return Err(Diagnostic::error(DiagKind::Config, pos, e.message().trim_end().to_string()));
        }
    };
    let reader = Reader { text, file, li };
    for (k, v) in parsed.get_ref().iter() {
        if k.get_ref().as_ref() != "about" {
            continue;
        }
        return match v.get_ref() {
            DeValue::Table(t) => reader.about(t, v.span().start),
            other => reader.fail(v.span().start, format!("'about' must be a table, got {}", type_name(other))),
        };
    }
    Err(Diagnostic::error(DiagKind::Config, Some(SourcePos::new(file, 1, 1)), "the preset has no [about] table"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[[bar]]
name = "main"

[about]
title = "Glass"
description = "A floating bar."
schemes = ["dark", "light"]
tune = [{ var = "accent", label = "Accent" }, { var = "fg", label = "Text" }]

[about.contrast]
text = ["fg on bg bg+pill", "accent-fg on accent"]
icon = ["dim on row"]
line = ["accent-line on panel"]
decorative = ["overlay-dim"]
"#;

    fn err(text: &str) -> String {
        parse_about(text, "p.toml").unwrap_err().to_string()
    }

    #[test]
    fn a_full_table_reads_back_field_by_field() {
        let a = parse_about(GOOD, "p.toml").unwrap();
        assert_eq!((a.title.as_str(), a.description.as_str()), ("Glass", "A floating bar."));
        assert_eq!(a.schemes, vec![Scheme::Dark, Scheme::Light]);
        assert_eq!(a.tune.len(), 2);
        assert_eq!((a.tune[1].var.as_str(), a.tune[1].label.as_str()), ("fg", "Text"));
        assert_eq!(a.contrast.text, ["fg on bg bg+pill", "accent-fg on accent"]);
        assert_eq!((a.contrast.icon.len(), a.contrast.line.len(), a.contrast.decorative.len()), (1, 1, 1));
    }

    #[test]
    fn a_missing_table_or_field_is_named_with_its_place() {
        assert!(err("[[bar]]\nname = \"x\"\n").contains("p.toml:1:1"));
        assert!(err("a = 1").contains("no [about]"));
        let no_title = GOOD.replace("title = \"Glass\"\n", "");
        let at = err(&no_title);
        assert!(at.contains("p.toml:5:") && at.contains("needs 'title'"), "{at}");
    }

    #[test]
    fn limits_and_shapes_are_checked_with_the_line() {
        let long = err(&GOOD.replace("Glass", &"x".repeat(33)));
        assert!(long.contains("p.toml:6:") && long.contains("33 characters"), "{long}");
        assert!(err(&GOOD.replace("A floating bar.", &"y".repeat(101))).contains("101 characters"));
        assert!(err(&GOOD.replace("\"dark\", \"light\"", "\"dark\", \"dark\"")).contains("listed twice"));
        assert!(err(&GOOD.replace("\"dark\", \"light\"", "")).contains("at least one"));
        assert!(err(&GOOD.replace("\"dark\", \"light\"", "\"dusk\"")).contains("not a scheme"));
        assert!(
            err(&GOOD.replace("{ var = \"accent\", label = \"Accent\" }, ", "")).contains("must include the accent")
        );
        assert!(
            err(&GOOD.replace("label = \"Text\"", "label = \"A label far too long to show\"")).contains("limit is 20")
        );
        assert!(err(&GOOD.replace("var = \"fg\"", "var = \"--fg\"")).contains("custom property name"));
        assert!(err(&GOOD.replace("var = \"fg\"", "var = \"accent\"")).contains("listed twice"));
        assert!(err(&GOOD.replace("fg on bg bg+pill", "fg bg")).contains("'<colour> on <surface>"));
        assert!(err(&GOOD.replace("fg on bg bg+pill", "fg on")).contains("in 'text'"));
        assert!(err(&GOOD.replace("overlay-dim", "overlay dim")).contains("in 'decorative'"));
        assert!(err(&GOOD.replace("description =", "descripton =")).contains("did you mean 'description'"));
        assert!(err(&GOOD.replace("text = [", "txt = [")).contains("unknown key 'txt' in [about.contrast]"));
        assert!(err("[about]\ntitle = 3\n").contains("must be a string, got an integer"));
        assert!(err("about = 3\n").contains("'about' must be a table"));
        assert!(err("[about\n").contains("p.toml:"));
    }

    #[test]
    fn the_title_and_description_are_one_line() {
        assert!(err(&GOOD.replace("title = \"Glass\"", "title = \"Gl\\nass\"")).contains("one line"));
        assert!(err(&GOOD.replace("title = \"Glass\"", "title = \"  \"")).contains("must not be empty"));
    }

    #[test]
    fn pair_lines_split_into_a_colour_and_surfaces() {
        assert_eq!(
            split_pair_line("fg on seg0 seg1 row+audio-fill-drag"),
            Some(("fg", vec!["seg0", "seg1", "row+audio-fill-drag"]))
        );
        assert_eq!(split_pair_line("fg on"), None);
        assert_eq!(split_pair_line("fg bg"), None);
        assert_eq!(split_pair_line("fg on a++b"), None);
        assert_eq!(split_pair_line("Fg on bg"), None);
        assert_eq!(surface_layers("bg+zone+pill"), Some(vec!["bg", "zone", "pill"]));
    }
}
