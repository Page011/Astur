//! Text helpers of the edit engine: line arithmetic, the file's line ending,
//! and the bookkeeping that turns a series of edits, each made against the
//! text the one before left, into one list of splices against the text the
//! file started with.

use super::Splice;

/// The line ending inserted lines use: CRLF when the file has one anywhere,
/// else LF. Existing line endings are never rewritten.
pub(crate) fn eol_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Offset of the first byte of the line holding `pos`.
pub(crate) fn line_start(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Offset just after the line break of the line holding `pos`, or the end of
/// the text for a last line with no break.
pub(crate) fn line_end_incl(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    text[pos..].find('\n').map_or(text.len(), |i| pos + i + 1)
}

/// Offset of the line break of the line holding `pos` (before a `\r`), or the
/// end of the text.
pub(crate) fn line_end_excl(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    let end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    if end > pos && text.as_bytes()[end - 1] == b'\r' { end - 1 } else { end }
}

/// 1-based line number of `pos`.
pub(crate) fn line_number(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].bytes().filter(|b| *b == b'\n').count() + 1
}

/// The leading blanks of the line holding `pos`.
pub(crate) fn indent_at(text: &str, pos: usize) -> &str {
    let start = line_start(text, pos);
    let rest = &text[start..];
    let n = rest.bytes().take_while(|b| *b == b' ' || *b == b'\t').count();
    &rest[..n]
}

/// True when `text` between `a` and `b` holds only blanks (no line break).
pub(crate) fn only_blanks(text: &str, a: usize, b: usize) -> bool {
    a <= b && text[a..b].bytes().all(|c| c == b' ' || c == b'\t')
}

/// The text ends with a line break, or is empty.
pub(crate) fn ends_with_break(text: &str) -> bool {
    text.is_empty() || text.ends_with('\n')
}

/// One piece of the text being edited: bytes of the original, or new text.
#[derive(Clone, Debug)]
enum Piece {
    Orig(usize, usize),
    New(String),
}

impl Piece {
    fn len(&self) -> usize {
        match self {
            Piece::Orig(a, b) => b - a,
            Piece::New(s) => s.len(),
        }
    }
}

/// A text that is edited step by step. Each step's splices are in the
/// coordinates of the text as it stands; `splices` reports everything done
/// so far against the original.
#[derive(Clone, Debug)]
pub(crate) struct Doc {
    original: String,
    pieces: Vec<Piece>,
    pub text: String,
}

impl Doc {
    pub(crate) fn new(text: &str) -> Doc {
        Doc { original: text.to_string(), pieces: vec![Piece::Orig(0, text.len())], text: text.to_string() }
    }

    /// Applies `splices` (sorted by start, not overlapping, in the current
    /// text's coordinates).
    pub(crate) fn apply(&mut self, splices: &[Splice]) {
        for s in splices.iter().rev() {
            self.replace(s.start, s.end, &s.text);
        }
        self.text = apply_splices(&self.text, splices);
    }

    fn replace(&mut self, start: usize, end: usize, text: &str) {
        let mut out: Vec<Piece> = Vec::with_capacity(self.pieces.len() + 2);
        let mut pos = 0usize;
        let mut inserted = false;
        for piece in std::mem::take(&mut self.pieces) {
            let (ps, pe) = (pos, pos + piece.len());
            pos = pe;
            if ps < start {
                out.push(slice(&piece, 0, start.min(pe) - ps));
            }
            if !inserted && pe >= start {
                push_new(&mut out, text);
                inserted = true;
            }
            if pe > end {
                out.push(slice(&piece, end.max(ps) - ps, pe - ps));
            }
        }
        if !inserted {
            push_new(&mut out, text);
        }
        self.pieces = merge_news(out);
    }

    /// Everything done so far, against the original text, in order. A splice
    /// that puts back the very bytes it removes is left out.
    pub(crate) fn splices(&self) -> Vec<Splice> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        let mut pending = String::new();
        let flush = |from: usize, to: usize, pending: &mut String, out: &mut Vec<Splice>| {
            if !pending.is_empty() || to > from {
                if self.original[from..to] != **pending {
                    out.push(Splice { start: from, end: to, text: std::mem::take(pending) });
                } else {
                    pending.clear();
                }
            }
        };
        for piece in &self.pieces {
            match piece {
                Piece::New(s) => pending.push_str(s),
                Piece::Orig(a, b) => {
                    flush(cursor, *a, &mut pending, &mut out);
                    cursor = *b;
                }
            }
        }
        flush(cursor, self.original.len(), &mut pending, &mut out);
        out
    }

    pub(crate) fn original(&self) -> &str {
        &self.original
    }
}

fn push_new(out: &mut Vec<Piece>, text: &str) {
    if !text.is_empty() {
        out.push(Piece::New(text.to_string()));
    }
}

/// The part of `piece` from offset `from` to offset `to` (inside the piece).
fn slice(piece: &Piece, from: usize, to: usize) -> Piece {
    match piece {
        Piece::Orig(a, _) => Piece::Orig(a + from, a + to),
        Piece::New(s) => Piece::New(s[from..to].to_string()),
    }
}

fn merge_news(pieces: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::with_capacity(pieces.len());
    for p in pieces {
        match (out.last_mut(), p) {
            (Some(Piece::New(prev)), Piece::New(next)) => prev.push_str(&next),
            (Some(Piece::Orig(_, e)), Piece::Orig(a, b)) if *e == a => *e = b,
            (_, p) => out.push(p),
        }
    }
    out
}

/// Applies splices to a text. They must be sorted and not overlap.
pub fn apply_splices(text: &str, splices: &[Splice]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for s in splices {
        out.push_str(&text[at..s.start]);
        out.push_str(&s.text);
        at = s.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(start: usize, end: usize, text: &str) -> Splice {
        Splice { start, end, text: text.to_string() }
    }

    #[test]
    fn line_arithmetic() {
        let t = "ab\r\ncd\nef";
        assert_eq!((line_start(t, 5), line_end_incl(t, 5), line_end_excl(t, 1)), (4, 7, 2));
        assert_eq!((line_end_incl(t, 8), line_end_excl(t, 8)), (9, 9));
        assert_eq!(line_number(t, 5), 2);
        assert_eq!(eol_of(t), "\r\n");
        assert_eq!(eol_of("a\nb"), "\n");
        assert_eq!(indent_at("x\n  \ty = 1", 6), "  \t");
    }

    #[test]
    fn steps_compose_into_splices_against_the_original() {
        let mut d = Doc::new("0123456789");
        d.apply(&[sp(2, 4, "AB_")]);
        assert_eq!(d.text, "01AB_456789");
        // A second step edits inside the text the first step wrote and beside it.
        d.apply(&[sp(3, 4, "bb"), sp(8, 9, "")]);
        assert_eq!(d.text, "01AbB_45689".replace("AbB_", "Abb_"));
        let splices = d.splices();
        assert_eq!(apply_splices(d.original(), &splices), d.text);
        assert!(splices.windows(2).all(|w| w[0].end <= w[1].start), "{splices:?}");
        assert_eq!(splices.len(), 2, "{splices:?}");
    }

    #[test]
    fn an_insert_at_the_start_and_end_and_a_deletion() {
        let mut d = Doc::new("abc");
        d.apply(&[sp(0, 0, "<"), sp(3, 3, ">")]);
        d.apply(&[sp(1, 2, "")]);
        assert_eq!(d.text, "<bc>");
        assert_eq!(apply_splices("abc", &d.splices()), "<bc>");
        assert_eq!(
            d.splices(),
            vec![sp(0, 0, "<"), sp(0, 1, ""), sp(3, 3, ">")].into_iter().fold(Vec::new(), |mut v: Vec<Splice>, s| {
                if let Some(last) = v.last_mut().filter(|l| l.end == s.start) {
                    last.end = s.end;
                    last.text.push_str(&s.text);
                } else {
                    v.push(s);
                }
                v
            })
        );
    }

    #[test]
    fn a_change_that_puts_the_same_bytes_back_is_no_splice() {
        let mut d = Doc::new("height = 36\n");
        d.apply(&[sp(9, 11, "40")]);
        d.apply(&[sp(9, 11, "36")]);
        assert_eq!(d.text, "height = 36\n");
        assert!(d.splices().is_empty());
    }

    #[test]
    fn every_original_byte_outside_the_splices_is_kept() {
        let original = "a = 1 # one\nb = 2\n\nc = [\"x\", \"y\"]\n";
        let mut d = Doc::new(original);
        let at = d.text.find("2").unwrap();
        d.apply(&[sp(at, at + 1, "22")]);
        let at = d.text.find("\"y\"").unwrap();
        d.apply(&[sp(at - 2, at + 3, "")]);
        let at = d.text.find("a = 1").unwrap();
        d.apply(&[sp(at, at, "z = 0\n")]);
        let splices = d.splices();
        let mut cursor = 0;
        let mut kept = String::new();
        for s in &splices {
            kept.push_str(&original[cursor..s.start]);
            cursor = s.end;
        }
        kept.push_str(&original[cursor..]);
        let mut expect = original.to_string();
        // What is outside the splices is exactly the original minus the ranges.
        for s in splices.iter().rev() {
            expect.replace_range(s.start..s.end, "");
        }
        assert_eq!(kept, expect);
        assert_eq!(apply_splices(original, &splices), d.text);
    }
}
