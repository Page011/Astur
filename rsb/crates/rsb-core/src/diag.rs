//! Diagnostics and the error plate.
//!
//! Every problem a user can cause (config, style, script, permission) and
//! every silent failure (a stopped script, a stale service, a render or pipe
//! failure) becomes one `Diagnostic`. It is written to the log in full and,
//! when it needs the user, drawn on the bar as a `Plate`.

use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Logged and shown, but the new config or style is still applied
    /// (unknown key or property, bad value: that one line is dropped).
    Warning,
    /// The file could not be used at all (syntax error): the last good
    /// version stays in effect.
    Error,
}

/// Where in a user file. `line` and `col` are 1-based; `col` 0 means the
/// whole line, `line` 0 the whole file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourcePos {
    /// Path relative to the config folder with `/` separators, e.g.
    /// `style.css` or `widgets/my/weather.luau`. Never an absolute path.
    pub file: Arc<str>,
    pub line: u32,
    pub col: u32,
}

impl SourcePos {
    pub fn new(file: &str, line: u32, col: u32) -> SourcePos {
        SourcePos { file: Arc::from(file), line, col }
    }
}

impl fmt::Display for SourcePos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.line, self.col) {
            (0, _) => write!(f, "{}", self.file),
            (l, 0) => write!(f, "{}:{l}", self.file),
            (l, c) => write!(f, "{}:{l}:{c}", self.file),
        }
    }
}

/// What a diagnostic is about. Picks the plate's key word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagKind {
    Config,
    Style,
    Script,
    Permission,
    /// A script was stopped for its time or memory budget.
    Stopped,
    Service,
    Render,
    Pipe,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagKind,
    pub pos: Option<SourcePos>,
    /// Short, lower case, says what is wrong and, where it can, what to do:
    /// "unknown property 'colr'", "my/weather needs net:api.open-meteo.com;
    /// add it to permissions". Never holds window titles, media names,
    /// network names, user names or absolute paths.
    pub message: String,
}

impl Diagnostic {
    pub fn error(kind: DiagKind, pos: Option<SourcePos>, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Error, kind, pos, message: message.into() }
    }

    pub fn warning(kind: DiagKind, pos: Option<SourcePos>, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, kind, pos, message: message.into() }
    }

    /// One line for the bar: key word, position, message. Examples:
    /// `RSB ERROR - style.css:12:5 unknown property 'colr'`,
    /// `PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions`,
    /// `STOPPED - my/weather ran over 10 ms; fix it and save`.
    pub fn plate_text(&self) -> String {
        let word = plate_word(self.kind);
        match &self.pos {
            Some(p) if !matches!(self.kind, DiagKind::Permission | DiagKind::Stopped) => {
                format!("{word} - {p} {}", self.message)
            }
            _ => format!("{word} - {}", self.message),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        match &self.pos {
            Some(p) => write!(f, "{sev}: {p}: {}", self.message),
            None => write!(f, "{sev}: {}", self.message),
        }
    }
}

/// Characters that are not control characters but split or reorder a line for a
/// reader: line and paragraph separators and the bidirectional marks and
/// overrides. Logs write them as spaces or escapes.
pub fn is_invisible_control(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Upper-case key word that starts a plate.
pub fn plate_word(kind: DiagKind) -> &'static str {
    match kind {
        DiagKind::Config | DiagKind::Style | DiagKind::Script => "RSB ERROR",
        DiagKind::Permission => "PERMISSION",
        DiagKind::Stopped => "STOPPED",
        DiagKind::Service => "SERVICE ERROR",
        DiagKind::Render => "RENDER ERROR",
        DiagKind::Pipe => "PIPE ERROR",
    }
}

/// What the bar draws for a problem. A bar plate sits at the start of the
/// left zone; a widget plate replaces that widget's content.
#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub kind: DiagKind,
    /// `Diagnostic::plate_text` of the first problem.
    pub text: String,
    /// How many more problems are in the log; drawn as ` (+N more, see log)`.
    pub more: usize,
}

impl Plate {
    /// The plate for a batch of diagnostics: the first error, else the first
    /// warning. None when the batch is empty.
    pub fn from_diagnostics(diags: &[Diagnostic]) -> Option<Plate> {
        let first = diags.iter().find(|d| d.severity == Severity::Error).or_else(|| diags.first())?;
        Some(Plate { kind: first.kind, text: first.plate_text(), more: diags.len() - 1 })
    }

    /// The full line drawn on the bar.
    pub fn display_text(&self) -> String {
        if self.more == 0 { self.text.clone() } else { format!("{} (+{} more, see log)", self.text, self.more) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plate_text_matches_the_documented_copy() {
        let d =
            Diagnostic::warning(DiagKind::Style, Some(SourcePos::new("style.css", 12, 5)), "unknown property 'colr'");
        assert_eq!(d.plate_text(), "RSB ERROR - style.css:12:5 unknown property 'colr'");
        let p = Diagnostic::error(
            DiagKind::Permission,
            None,
            "my/weather needs net:api.open-meteo.com; add it to permissions",
        );
        assert_eq!(p.plate_text(), "PERMISSION - my/weather needs net:api.open-meteo.com; add it to permissions");
    }

    #[test]
    fn plate_prefers_errors_and_counts_the_rest() {
        let w = Diagnostic::warning(DiagKind::Config, None, "a");
        let e = Diagnostic::error(DiagKind::Config, None, "b");
        let plate = Plate::from_diagnostics(&[w, e]).unwrap();
        assert_eq!(plate.display_text(), "RSB ERROR - b (+1 more, see log)");
        assert!(Plate::from_diagnostics(&[]).is_none());
    }
}
