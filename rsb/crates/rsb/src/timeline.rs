//! The timeline file `rsb render --timeline` plays: one line per step, in
//! virtual milliseconds from the start of the run.
//!
//! ```text
//! <ms> pointer <surface> move|down|up|wheel|hwheel|leave <x> <y> [left|right|middle|<notches>]
//! <ms> key <popup> <Key> [down|up] [ctrl+shift+alt] [repeat]
//! <ms> text <popup> <text>                      (\uXXXX, \s for a space, \\)
//! <ms> ime <popup> start | update <text> <cursor> | commit <text> | end
//! <ms> popup <name> open|close|toggle
//! <ms> outside                                  (a press outside every window)
//! <ms> capture <label> [<surface>|screen]       (writes <label>.png)
//! <ms> fake <service> <json object>             (sets those fields of the fake service's value)
//! ```
//!
//! A surface is a bar name (`main` or `bar:main`), `popup:<name>` or
//! `dim:<name>`. Blank lines and lines that start with `#` are ignored.

use rsb_core::services::ServiceId;

use crate::msg::SimAction;
use crate::sim;

/// Latest time a line may name, and most captures one file may ask for: the same limits a `--capture` run has.
pub const MAX_MS: u64 = crate::capture::MAX_MS;
pub const MAX_CAPTURES: usize = crate::capture::MAX_FRAMES as usize;
/// Most lines one file may have.
pub const MAX_LINES: usize = 20_000;

/// What a `capture` line takes a picture of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every bar and popup at its place on the screen, over the background.
    Screen,
    Bar(String),
    Popup(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Sim(SimAction),
    Capture { label: String, target: Target },
    Fake { service: ServiceId, fields: serde_json::Map<String, serde_json::Value> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub ms: u64,
    /// 1-based line number in the file.
    pub line: usize,
    pub step: Step,
}

/// Text before the first `: ` of a sub-parser's message is its own "line N" prefix.
fn without_prefix(e: &str) -> String {
    e.split_once(": ").map_or_else(|| e.to_string(), |(_, rest)| rest.to_string())
}

fn label_ok(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 64
        && !label.starts_with('.')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn popup_name(word: &str) -> Result<String, String> {
    let name = word.strip_prefix("popup:").unwrap_or(word);
    if name.is_empty() { Err(format!("'{word}' names no popup")) } else { Ok(name.to_string()) }
}

fn parse_target(word: &str) -> Result<Target, String> {
    if word == "screen" {
        return Ok(Target::Screen);
    }
    match sim::parse_surface(word, true)? {
        crate::msg::SimSurface::Bar(n) => Ok(Target::Bar(n)),
        crate::msg::SimSurface::Popup(n) => Ok(Target::Popup(n)),
        crate::msg::SimSurface::Dim(_) => Err("a dim window cannot be captured on its own; use screen".into()),
    }
}

/// Parses a whole timeline. The error names the line (`timeline line 7: ...`).
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = Vec::new();
    let mut captures = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |m: &str| format!("timeline line {line_no}: {m}");
        if out.len() >= MAX_LINES {
            return Err(at(&format!("more than {MAX_LINES} steps")));
        }
        let mut words = line.split_whitespace();
        let ms_word = words.next().unwrap_or_default();
        let ms: u64 = ms_word.parse().map_err(|_| at(&format!("'{ms_word}' is not a time in milliseconds")))?;
        if ms > MAX_MS {
            return Err(at(&format!("{ms} ms is past the {MAX_MS} ms limit")));
        }
        let kind = words.next().ok_or_else(|| at("no step after the time"))?;
        let rest: Vec<&str> = words.collect();
        let step = match kind {
            "pointer" => {
                if rest.len() < 4 {
                    return Err(at("use pointer <surface> move|down|up|wheel|hwheel|leave <x> <y> [button|notches]"));
                }
                let one = format!("{ms} {}", rest.join(" "));
                let mut parsed = sim::parse_pointer_script(&one).map_err(|e| at(&without_prefix(&e)))?;
                Step::Sim(parsed.remove(0).1)
            }
            "key" | "text" | "ime" => {
                let (popup, args) = rest.split_first().ok_or_else(|| at(&format!("use {kind} <popup> ...")))?;
                let name = popup_name(popup).map_err(|e| at(&e))?;
                // The text of a `text` step is one word: its spaces are written `\s`.
                let one = format!("{ms} popup:{name} {kind} {}", args.join(" "));
                let mut parsed = sim::parse_key_script(&one).map_err(|e| at(&without_prefix(&e)))?;
                Step::Sim(parsed.remove(0).1)
            }
            "popup" => match rest.as_slice() {
                [name, action] => {
                    let action = rsb_core::popup::PopupAction::from_name(action)
                        .ok_or_else(|| at(&format!("'{action}' is not open, close or toggle")))?;
                    Step::Sim(SimAction::Popup { name: popup_name(name).map_err(|e| at(&e))?, action })
                }
                _ => return Err(at("use popup <name> open|close|toggle")),
            },
            "outside" => {
                if !rest.is_empty() {
                    return Err(at("outside takes no arguments"));
                }
                Step::Sim(SimAction::OutsidePress)
            }
            "capture" => {
                let (label, target) = match rest.as_slice() {
                    [label] => (*label, Target::Screen),
                    [label, surface] => (*label, parse_target(surface).map_err(|e| at(&e))?),
                    _ => return Err(at("use capture <label> [<surface>|screen]")),
                };
                if !label_ok(label) {
                    return Err(at(&format!("'{label}' is not a file name (letters, digits, - _ . up to 64)")));
                }
                captures += 1;
                if captures > MAX_CAPTURES {
                    return Err(at(&format!("more than {MAX_CAPTURES} captures")));
                }
                Step::Capture { label: label.to_string(), target }
            }
            "fake" => {
                let after_ms = line.split_once(char::is_whitespace).map_or("", |x| x.1).trim_start();
                let after_kind = after_ms.split_once(char::is_whitespace).map_or("", |x| x.1).trim_start();
                let (name, json) = after_kind.split_once(char::is_whitespace).unwrap_or((after_kind, ""));
                let service = ServiceId::from_name(name).ok_or_else(|| {
                    let all: Vec<&str> = ServiceId::ALL.iter().map(|s| s.name()).collect();
                    at(&format!("'{name}' is not a service ({})", all.join(", ")))
                })?;
                let value: serde_json::Value =
                    serde_json::from_str(json.trim()).map_err(|e| at(&format!("the value is not JSON: {e}")))?;
                let serde_json::Value::Object(fields) = value else {
                    return Err(at("the value must be a JSON object"));
                };
                Step::Fake { service, fields }
            }
            other => {
                return Err(at(&format!(
                    "unknown step '{other}' (pointer, key, text, ime, popup, outside, capture, fake)"
                )));
            }
        };
        out.push(Entry { ms, line: line_no, step });
    }
    out.sort_by_key(|e| e.ms);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use rsb_core::hit::{Button, PointerEvent, PointerKind};
    use rsb_core::key::{Key, KeyEvent, TextInput};
    use rsb_core::popup::PopupAction;

    use super::*;
    use crate::msg::SimSurface;

    fn one(line: &str) -> Step {
        let mut v = parse(line).unwrap();
        assert_eq!(v.len(), 1, "{line}");
        v.remove(0).step
    }

    fn err(text: &str) -> String {
        parse(text).unwrap_err()
    }

    #[test]
    fn every_kind_of_line_parses() {
        assert_eq!(
            one("120 pointer main move 600 12"),
            Step::Sim(SimAction::Pointer {
                surface: SimSurface::Bar("main".into()),
                event: PointerEvent { kind: PointerKind::Move, x: 600.0, y: 12.0 }
            })
        );
        assert_eq!(
            one("5 pointer popup:dashboard down 10 20 right"),
            Step::Sim(SimAction::Pointer {
                surface: SimSurface::Popup("dashboard".into()),
                event: PointerEvent { kind: PointerKind::Down(Button::Right), x: 10.0, y: 20.0 }
            })
        );
        assert_eq!(
            one("5 pointer dim:power up 1 2"),
            Step::Sim(SimAction::Pointer {
                surface: SimSurface::Dim("power".into()),
                event: PointerEvent { kind: PointerKind::Up(Button::Left), x: 1.0, y: 2.0 }
            })
        );
        assert!(matches!(
            one("5 pointer bar:main wheel 3 4 -2"),
            Step::Sim(SimAction::Pointer { event: PointerEvent { kind: PointerKind::Wheel(n), .. }, .. }) if n == -2.0
        ));
        assert!(matches!(
            one("5 pointer main leave 0 0"),
            Step::Sim(SimAction::Pointer { event: PointerEvent { kind: PointerKind::Leave, .. }, .. })
        ));
        assert_eq!(
            one("30 key launcher Enter"),
            Step::Sim(SimAction::Key {
                popup: "launcher".into(),
                event: KeyEvent { key: Key::Enter, mods: Default::default(), down: true, repeat: false }
            })
        );
        assert!(matches!(
            one("30 key popup:launcher a up ctrl+shift repeat"),
            Step::Sim(SimAction::Key { event: KeyEvent { key: Key::Char('a'), down: false, repeat: true, .. }, .. })
        ));
        assert_eq!(
            one("40 text launcher b\\st"),
            Step::Sim(SimAction::Text { popup: "launcher".into(), input: TextInput::Chars("b t".into()) })
        );
        assert_eq!(
            one("40 ime launcher update \\u4e2d 1"),
            Step::Sim(SimAction::Text {
                popup: "launcher".into(),
                input: TextInput::CompositionUpdate { text: "\u{4e2d}".into(), cursor: 1 }
            })
        );
        assert_eq!(
            one("50 ime launcher start"),
            Step::Sim(SimAction::Text { popup: "launcher".into(), input: TextInput::CompositionStart })
        );
        assert_eq!(
            one("60 popup dashboard open"),
            Step::Sim(SimAction::Popup { name: "dashboard".into(), action: PopupAction::Open })
        );
        assert_eq!(
            one("60 popup popup:dashboard toggle"),
            Step::Sim(SimAction::Popup { name: "dashboard".into(), action: PopupAction::Toggle })
        );
        assert_eq!(one("70 outside"), Step::Sim(SimAction::OutsidePress));
        assert_eq!(one("80 capture a1"), Step::Capture { label: "a1".into(), target: Target::Screen });
        assert_eq!(one("80 capture a2 screen"), Step::Capture { label: "a2".into(), target: Target::Screen });
        assert_eq!(one("80 capture a3 main"), Step::Capture { label: "a3".into(), target: Target::Bar("main".into()) });
        assert_eq!(
            one("80 capture a4.x popup:dashboard"),
            Step::Capture { label: "a4.x".into(), target: Target::Popup("dashboard".into()) }
        );
        match one(r#"90 fake battery {"percent": 100, "charging": true}"#) {
            Step::Fake { service, fields } => {
                assert_eq!(service, ServiceId::Battery);
                assert_eq!(fields["percent"], 100);
                assert_eq!(fields["charging"], true);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn json_keeps_its_spaces_and_hash_signs() {
        match one(r##"1 fake media {"title": "a  b #c"}"##) {
            Step::Fake { fields, .. } => assert_eq!(fields["title"], "a  b #c"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blank_lines_and_comments_are_skipped_and_steps_come_out_in_time_order() {
        let v = parse("# a comment\n\n300 outside\n  100 popup x open\n200 capture z\n").unwrap();
        assert_eq!(v.iter().map(|e| e.ms).collect::<Vec<_>>(), [100, 200, 300]);
        assert_eq!(v.iter().map(|e| e.line).collect::<Vec<_>>(), [4, 5, 3], "line numbers are the file's");
    }

    #[test]
    fn bad_lines_are_refused_with_their_line_number() {
        let cases: &[(&str, &str)] = &[
            ("soon outside", "'soon' is not a time in milliseconds"),
            ("5", "no step after the time"),
            ("5 wiggle", "unknown step 'wiggle'"),
            ("999999 outside", "past the 120000 ms limit"),
            ("5 pointer main move 1", "use pointer <surface>"),
            ("5 pointer main nudge 1 2", "unknown event 'nudge'"),
            ("5 pointer main move x 2", "x is not a number"),
            ("5 pointer main down 1 2 purple", "'purple' is not left, right or middle"),
            ("5 pointer dim: move 1 2", "names no surface"),
            ("5 key", "use key <popup>"),
            ("5 key launcher", "key needs a key name"),
            ("5 key launcher Hyper", "'Hyper' is not a key name"),
            ("5 key launcher a sideways", "is not down, up, repeat or ctrl+shift+alt"),
            ("5 text launcher", "text needs the text"),
            ("5 text launcher a\\q", "unknown escape"),
            ("5 ime launcher update x", "use ime start"),
            ("5 popup x", "use popup <name>"),
            ("5 popup x sideways", "'sideways' is not open, close or toggle"),
            ("5 outside now", "outside takes no arguments"),
            ("5 capture", "use capture <label>"),
            ("5 capture a b c", "use capture <label>"),
            ("5 capture ../x", "'../x' is not a file name"),
            ("5 capture .hidden", "is not a file name"),
            ("5 capture a dim:power", "cannot be captured on its own"),
            ("5 fake battery", "the value is not JSON"),
            ("5 fake battery [1]", "must be a JSON object"),
            ("5 fake toaster {}", "'toaster' is not a service"),
            ("5 fake", "is not a service"),
        ];
        for (text, want) in cases {
            let e = err(&format!("\n# first\n{text}\n"));
            assert!(e.starts_with("timeline line 3: "), "{text}: {e}");
            assert!(e.contains(want), "{text}: {e}");
        }
    }

    #[test]
    fn the_capture_and_line_limits_are_enforced() {
        let many = (0..=MAX_CAPTURES).map(|i| format!("{i} capture c{i}\n")).collect::<String>();
        let e = err(&many);
        assert!(e.contains("more than 2000 captures"), "{e}");
        assert!(e.starts_with(&format!("timeline line {}: ", MAX_CAPTURES + 1)), "{e}");
        let ok = (0..MAX_CAPTURES).map(|i| format!("{} capture c{i}\n", i % 100)).collect::<String>();
        assert_eq!(parse(&ok).unwrap().len(), MAX_CAPTURES);
        let lines = "1 outside\n".repeat(MAX_LINES + 1);
        assert!(err(&lines).contains("more than 20000 steps"));
    }
}
