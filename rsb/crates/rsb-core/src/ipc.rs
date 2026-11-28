//! Command protocol between `rsb <command>` and the running bar.
//!
//! One request and one reply per pipe connection. A request is one line of
//! printable ASCII (0x20-0x7E) ending in `\n`, at most `MAX_REQUEST_BYTES`;
//! a reply is `ok <text>\n` or `err <text>\n`, at most `MAX_REPLY_BYTES`.
//! The running bar treats every request as untrusted: anything that does not
//! decode exactly is refused with an `err` reply and never acted on.
//!
//! Requests:
//!
//! ```text
//! reload
//! quit
//! toggle               every bar
//! toggle <bar>         one bar by name
//! status               reply text is one line of JSON (ASCII, \u escapes)
//! trigger <event>      event name: a-z 0-9 _ . -, 1..=64 chars
//! trigger <event> <json>   the rest of the line is JSON, non-ASCII escaped
//! popup <name> [open|close|toggle]
//! applied              the file versions the bar shows (JSON, `contract::AppliedReply`)
//! settings <json>      one settings request (JSON, `contract::Request`), non-ASCII escaped
//! inspect [from=<n>]   a page of the widget summary (JSON)
//! inspect <widget> [bar=<name>] [depth=<n>] [from=<n>] [style] [redact]
//! ```
//!
//! In an `inspect` line the first word is the widget unless it has an `=`,
//! so a line without a widget starts with `from=`; `style` and `redact`
//! are flags only after the first word.

use std::fmt;

use crate::config::{WidgetRef, is_valid_bar_name};

pub const MAX_REQUEST_BYTES: usize = 4096;
pub const MAX_REPLY_BYTES: usize = 65536;
pub const MAX_EVENT_NAME: usize = 64;
/// Deepest element tree `inspect` shows; also its default.
pub const MAX_INSPECT_DEPTH: u8 = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Reload,
    Quit,
    /// None = every bar.
    Toggle(Option<String>),
    Status,
    /// `json` is checked to parse as JSON by `decode`; kept as text.
    Trigger {
        event: String,
        json: Option<String>,
    },
    /// `rsb popup <name> [open|close|toggle]` (toggle when left out): the
    /// popup with that name on the monitor under the pointer.
    Popup {
        name: String,
        action: crate::popup::PopupAction,
    },
    /// The file versions the bar shows.
    Applied,
    /// One settings request as JSON text. `decode` checks the line, not the
    /// JSON: a request that does not parse gets a settings error reply.
    Settings(String),
    /// What each widget draws and costs. `widget` None is the summary of
    /// every widget, paged from the `from`th.
    Inspect {
        widget: Option<WidgetRef>,
        bar: Option<String>,
        from: u32,
        depth: u8,
        style: bool,
        redact: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Ok(String),
    Err(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    TooLong { len: usize, limit: usize },
    NotAscii,
    Empty,
    UnknownCommand(String),
    WrongArgs { command: &'static str, expected: &'static str },
    BadBarName(String),
    BadEventName(String),
    BadJson(String),
    BadReply,
    BadWidgetName(String),
    BadInspectArg { arg: String, why: &'static str },
}

/// How much of a refused client word is repeated back.
const ECHO_MAX: usize = 32;

/// The client's own text, cut short.
fn shown(text: &str) -> String {
    if text.chars().count() <= ECHO_MAX {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(ECHO_MAX).collect();
    cut.push_str("...");
    cut
}

impl fmt::Display for IpcError {
    /// Messages a person can act on, e.g. `usage: rsb toggle [bar]`,
    /// `unknown command 'x' (reload, quit, toggle, status, trigger)`.
    /// Echoed client text is cut to 32 chars.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::TooLong { len, limit } => write!(f, "command is {len} bytes, limit {limit}"),
            IpcError::NotAscii => f.write_str("command must be printable ASCII on one line"),
            IpcError::Empty => f.write_str("no command"),
            IpcError::UnknownCommand(c) => {
                let hint = crate::suggest::hint(c, COMMANDS);
                write!(f, "unknown command '{}'{hint} (reload, quit, toggle, status, trigger)", shown(c))
            }
            IpcError::WrongArgs { command, expected } => {
                let usage = format!("usage: rsb {command} {expected}");
                f.write_str(usage.trim_end())
            }
            IpcError::BadBarName(b) => write!(f, "bad bar name '{}': use a-z, 0-9, _ and -", shown(b)),
            IpcError::BadEventName(e) => {
                write!(f, "bad event name '{}': use a-z, 0-9, _, . and -, at most {MAX_EVENT_NAME}", shown(e))
            }
            IpcError::BadJson(m) => write!(f, "event data is not JSON: {m}"),
            IpcError::BadReply => f.write_str("malformed reply from the running bar"),
            IpcError::BadWidgetName(w) => {
                write!(f, "bad widget '{}': write it as in bar.toml, like clock or my/weather", shown(w))
            }
            IpcError::BadInspectArg { arg, why } => write!(f, "bad inspect argument '{}': {why}", shown(arg)),
        }
    }
}

impl std::error::Error for IpcError {}

fn is_printable(b: u8) -> bool {
    (0x20..=0x7E).contains(&b)
}

fn is_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_EVENT_NAME
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
}

/// Compact JSON with every non-ASCII character as a `\u` escape (surrogate
/// pairs above U+FFFF), so the request stays printable ASCII.
pub fn ascii_json(v: &serde_json::Value) -> String {
    let text = v.to_string();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

fn parse_json(text: &str) -> Result<serde_json::Value, IpcError> {
    // The parser's own message is not client text, so it is not cut short.
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| IpcError::BadJson(e.to_string()))?;
    // Delivery to a widget refuses deeper data later, after the bar has already said "ok".
    if crate::value::Value::from_json(&v).depth() > crate::value::MAX_DEPTH {
        return Err(IpcError::BadJson(format!("nested over {} levels", crate::value::MAX_DEPTH)));
    }
    Ok(v)
}

/// The commands a running bar answers, for "did you mean".
const COMMANDS: [&str; 9] =
    ["reload", "quit", "toggle", "status", "trigger", "popup", "applied", "settings", "inspect"];

/// The digits of an `inspect` number: no sign, at most 10 of them.
fn inspect_number(arg: &str, text: &str) -> Result<u32, IpcError> {
    let bad = |why| IpcError::BadInspectArg { arg: arg.to_string(), why };
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad("use a whole number"));
    }
    text.parse().map_err(|_| bad("the number is too large"))
}

/// `words[0]` is `inspect`.
fn build_inspect(words: &[&str]) -> Result<Command, IpcError> {
    let mut rest = &words[1..];
    let mut widget = None;
    if let Some(first) = rest.first().filter(|w| !w.contains('=')) {
        widget = Some(WidgetRef::parse(first).map_err(|_| IpcError::BadWidgetName(first.to_string()))?);
        rest = &rest[1..];
    }
    let (mut bar, mut from, mut depth, mut style, mut redact) = (None, None, None, false, false);
    for word in rest {
        let refuse = |why| IpcError::BadInspectArg { arg: word.to_string(), why };
        match word.split_once('=') {
            Some(("bar", v)) => {
                if bar.is_some() {
                    return Err(refuse("given twice"));
                }
                if !is_valid_bar_name(v) {
                    return Err(IpcError::BadBarName(v.to_string()));
                }
                bar = Some(v.to_string());
            }
            Some(("from", v)) => {
                if from.is_some() {
                    return Err(refuse("given twice"));
                }
                from = Some(inspect_number(word, v)?);
            }
            Some(("depth", v)) => {
                if depth.is_some() {
                    return Err(refuse("given twice"));
                }
                match u8::try_from(inspect_number(word, v)?) {
                    Ok(d) if (1..=MAX_INSPECT_DEPTH).contains(&d) => depth = Some(d),
                    _ => return Err(refuse("depth is 1 to 64")),
                }
            }
            None if *word == "style" && !style => style = true,
            None if *word == "redact" && !redact => redact = true,
            None if *word == "style" || *word == "redact" => return Err(refuse("given twice")),
            _ => return Err(refuse("unknown word")),
        }
    }
    Ok(Command::Inspect {
        widget,
        bar,
        from: from.unwrap_or(0),
        depth: depth.unwrap_or(MAX_INSPECT_DEPTH),
        style,
        redact,
    })
}

/// `words` is non-empty; `json` is the raw rest of a trigger line, if any.
fn build(words: &[&str], json: Option<String>) -> Result<Command, IpcError> {
    let simple = |command: &'static str, cmd: Command| {
        if words.len() == 1 { Ok(cmd) } else { Err(IpcError::WrongArgs { command, expected: "" }) }
    };
    match words[0] {
        "reload" => simple("reload", Command::Reload),
        "quit" => simple("quit", Command::Quit),
        "status" => simple("status", Command::Status),
        "toggle" => match words {
            [_] => Ok(Command::Toggle(None)),
            [_, bar] if is_valid_bar_name(bar) => Ok(Command::Toggle(Some(bar.to_string()))),
            [_, bar] => Err(IpcError::BadBarName(bar.to_string())),
            _ => Err(IpcError::WrongArgs { command: "toggle", expected: "[bar]" }),
        },
        "trigger" => match words {
            [_, event] if is_event_name(event) => Ok(Command::Trigger { event: event.to_string(), json }),
            [_, event] => Err(IpcError::BadEventName(event.to_string())),
            _ => Err(IpcError::WrongArgs { command: "trigger", expected: "<event> [json]" }),
        },
        "applied" => simple("applied", Command::Applied),
        "settings" => match json {
            Some(j) => Ok(Command::Settings(j)),
            None => Err(IpcError::WrongArgs { command: "settings", expected: "<json>" }),
        },
        "inspect" => build_inspect(words),
        "popup" => {
            let action = |a: &str| crate::popup::PopupAction::from_name(a);
            match words {
                [_, name] if is_event_name(name) => {
                    Ok(Command::Popup { name: name.to_string(), action: crate::popup::PopupAction::Toggle })
                }
                [_, name, a] if is_event_name(name) && action(a).is_some() => Ok(Command::Popup {
                    name: name.to_string(),
                    action: action(a).unwrap_or(crate::popup::PopupAction::Toggle),
                }),
                [_, name, ..] if !is_event_name(name) => Err(IpcError::BadEventName(name.to_string())),
                _ => Err(IpcError::WrongArgs { command: "popup", expected: "<name> [open|close|toggle]" }),
            }
        }
        other => Err(IpcError::UnknownCommand(other.to_string())),
    }
}

impl Command {
    /// From command-line words after the program name, e.g.
    /// `["trigger", "deploy", "{\"ok\":true}"]`. Non-ASCII JSON is
    /// re-encoded with `\u` escapes so it fits the wire.
    pub fn from_args(args: &[String]) -> Result<Command, IpcError> {
        let Some(first) = args.first() else {
            return Err(IpcError::Empty);
        };
        let cmd = if first == "trigger" && args.len() == 3 {
            let json = ascii_json(&parse_json(&args[2])?);
            build(&["trigger", &args[1]], Some(json))?
        } else if first == "settings" && args.len() == 2 {
            let json = ascii_json(&parse_json(&args[1])?);
            build(&["settings"], Some(json))?
        } else {
            let words: Vec<&str> = args.iter().map(String::as_str).collect();
            build(&words, None)?
        };
        let len = cmd.encode().len();
        if len > MAX_REQUEST_BYTES {
            return Err(IpcError::TooLong { len, limit: MAX_REQUEST_BYTES });
        }
        Ok(cmd)
    }

    /// Wire form with the trailing newline.
    pub fn encode(&self) -> String {
        match self {
            Command::Reload => "reload\n".into(),
            Command::Quit => "quit\n".into(),
            Command::Status => "status\n".into(),
            Command::Toggle(None) => "toggle\n".into(),
            Command::Toggle(Some(bar)) => format!("toggle {bar}\n"),
            Command::Trigger { event, json: None } => format!("trigger {event}\n"),
            Command::Trigger { event, json: Some(j) } => format!("trigger {event} {j}\n"),
            Command::Popup { name, action } => format!("popup {name} {}\n", action.name()),
            Command::Applied => "applied\n".into(),
            Command::Settings(json) => format!("settings {json}\n"),
            Command::Inspect { widget, bar, from, depth, style, redact } => {
                let mut line = String::from("inspect");
                match widget {
                    Some(w) => line.push_str(&format!(" {w}")),
                    // No widget: the first word has an `=`, so it is not read as one.
                    None => line.push_str(&format!(" from={from}")),
                }
                if widget.is_some() && *from != 0 {
                    line.push_str(&format!(" from={from}"));
                }
                if let Some(b) = bar {
                    line.push_str(&format!(" bar={b}"));
                }
                if *depth != MAX_INSPECT_DEPTH {
                    line.push_str(&format!(" depth={depth}"));
                }
                if *style {
                    line.push_str(" style");
                }
                if *redact {
                    line.push_str(" redact");
                }
                line.push('\n');
                line
            }
        }
    }

    /// Strict inverse of `encode`: length first, then characters, exactly
    /// one trailing newline, single spaces, known verb and arity.
    pub fn decode(bytes: &[u8]) -> Result<Command, IpcError> {
        let body = split_line(bytes, MAX_REQUEST_BYTES)?;
        if body.is_empty() {
            return Err(IpcError::Empty);
        }
        // Printable ASCII only, so this is valid UTF-8.
        let text = std::str::from_utf8(body).map_err(|_| IpcError::NotAscii)?;
        if let Some(rest) = text.strip_prefix("trigger ")
            && let Some((event, json)) = rest.split_once(' ')
        {
            if json.is_empty() || json.starts_with(' ') || event.is_empty() {
                return Err(IpcError::UnknownCommand(text.to_string()));
            }
            parse_json(json)?;
            return build(&["trigger", event], Some(json.to_string()));
        }
        if let Some(json) = text.strip_prefix("settings ") {
            if json.is_empty() || json.starts_with(' ') {
                return Err(IpcError::UnknownCommand(text.to_string()));
            }
            return build(&["settings"], Some(json.to_string()));
        }
        let words: Vec<&str> = text.split(' ').collect();
        if words.iter().any(|w| w.is_empty()) {
            // Leading, trailing or doubled space.
            return Err(IpcError::UnknownCommand(text.to_string()));
        }
        build(&words, None)
    }
}

/// Checks size and character set and returns the line without its newline.
fn split_line(bytes: &[u8], limit: usize) -> Result<&[u8], IpcError> {
    if bytes.len() > limit {
        return Err(IpcError::TooLong { len: bytes.len(), limit });
    }
    let Some((&last, body)) = bytes.split_last() else {
        return Err(IpcError::Empty);
    };
    if last != b'\n' || !body.iter().all(|b| is_printable(*b)) {
        return Err(IpcError::NotAscii);
    }
    Ok(body)
}

impl Reply {
    /// Non-printable characters replaced by `?`. An error text is cut to
    /// fit; an answer that does not fit (a status as JSON) becomes an error
    /// saying so, never a cut-off half of a document.
    pub fn encode(&self) -> String {
        let (prefix, text) = match self {
            Reply::Ok(t) if t.len() > MAX_REPLY_BYTES - "ok ".len() - 1 => {
                return Reply::Err(format!("the answer is longer than {MAX_REPLY_BYTES} bytes")).encode();
            }
            Reply::Ok(t) => ("ok ", t),
            Reply::Err(t) => ("err ", t),
        };
        let room = MAX_REPLY_BYTES - prefix.len() - 1;
        let mut out = String::with_capacity(prefix.len() + text.len().min(room) + 1);
        out.push_str(prefix);
        // Every kept character is one ASCII byte, so counting chars counts bytes.
        out.extend(text.chars().take(room).map(|c| if c.is_ascii() && is_printable(c as u8) { c } else { '?' }));
        out.push('\n');
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Reply, IpcError> {
        let body = split_line(bytes, MAX_REPLY_BYTES)?;
        let text = std::str::from_utf8(body).map_err(|_| IpcError::NotAscii)?;
        let rest = |prefix: &str| -> Option<String> {
            if text == prefix.trim_end() { Some(String::new()) } else { text.strip_prefix(prefix).map(str::to_string) }
        };
        if let Some(t) = rest("ok ") {
            Ok(Reply::Ok(t))
        } else if let Some(t) = rest("err ") {
            Ok(Reply::Err(t))
        } else {
            Err(IpcError::BadReply)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    fn all_commands() -> Vec<Command> {
        vec![
            Command::Reload,
            Command::Quit,
            Command::Status,
            Command::Toggle(None),
            Command::Toggle(Some("main".into())),
            Command::Trigger { event: "deploy".into(), json: None },
            Command::Trigger { event: "build.done".into(), json: Some("{\"ok\":true,\"n\":[1, 2]}".into()) },
            Command::Applied,
            Command::Settings("{\"verb\":\"get\"}".into()),
            Command::Settings("{\"changes\":[{\"op\":\"set\",\"key\":\"preset\",\"value\":\"a b\"}]}".into()),
            inspect(None, None, 0, 64, false, false),
            inspect(None, None, 3, 64, false, false),
            inspect(None, Some("main"), 0, 64, true, true),
            inspect(Some("clock"), None, 0, 64, false, false),
            inspect(Some("my/weather"), Some("main"), 0, 5, true, false),
            inspect(Some("clock#utc"), None, 7, 1, false, true),
            inspect(Some("style"), None, 0, 64, true, false),
        ]
    }

    fn inspect(widget: Option<&str>, bar: Option<&str>, from: u32, depth: u8, style: bool, redact: bool) -> Command {
        Command::Inspect {
            widget: widget.map(|w| WidgetRef::parse(w).unwrap()),
            bar: bar.map(str::to_string),
            from,
            depth,
            style,
            redact,
        }
    }

    #[test]
    fn every_command_round_trips() {
        for cmd in all_commands() {
            let wire = cmd.encode();
            assert!(wire.ends_with('\n') && wire.len() <= MAX_REQUEST_BYTES);
            assert_eq!(Command::decode(wire.as_bytes()), Ok(cmd), "{wire:?}");
        }
        assert_eq!(Command::Toggle(Some("main".into())).encode(), "toggle main\n");
    }

    #[test]
    fn from_args_matches_decode_and_escapes_json() {
        assert_eq!(Command::from_args(&[]), Err(IpcError::Empty));
        assert_eq!(Command::from_args(&words(&["toggle", "top"])), Ok(Command::Toggle(Some("top".into()))));
        let cmd = Command::from_args(&words(&["trigger", "say", "{\"text\": \"caf\u{e9} \u{1F600}\"}"])).unwrap();
        let wire = cmd.encode();
        assert_eq!(wire, "trigger say {\"text\":\"caf\\u00e9 \\ud83d\\ude00\"}\n");
        assert_eq!(Command::decode(wire.as_bytes()), Ok(cmd));
        let back: serde_json::Value = serde_json::from_str(&wire["trigger say ".len()..]).unwrap();
        assert_eq!(back["text"], "caf\u{e9} \u{1F600}");
        assert!(matches!(Command::from_args(&words(&["trigger", "x", "{nope"])), Err(IpcError::BadJson(_))));
        assert!(matches!(
            Command::from_args(&words(&["trigger", "x", "{}", "extra"])),
            Err(IpcError::WrongArgs { command: "trigger", .. })
        ));
        let big = format!("\"{}\"", "a".repeat(5000));
        assert!(matches!(Command::from_args(&words(&["trigger", "x", &big])), Err(IpcError::TooLong { .. })));
    }

    #[test]
    fn trigger_data_nested_too_deeply_is_refused_before_the_bar_says_ok() {
        let nested = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        let ok = Command::from_args(&words(&["trigger", "x", &nested(64)]));
        assert!(ok.is_ok(), "{ok:?}");
        for depth in [65, 100, 120] {
            let wire = format!("trigger x {}\n", nested(depth));
            let Err(IpcError::BadJson(why)) = Command::decode(wire.as_bytes()) else { panic!("depth {depth}") };
            assert!(why.contains("nested over 64 levels"), "{why}");
        }
    }

    #[test]
    fn json_and_command_mistakes_read_in_full_and_get_a_suggestion() {
        let Err(e) = Command::decode(b"trigger x {\"a\":1,}\n") else { panic!() };
        let text = e.to_string();
        assert!(text.starts_with("event data is not JSON: ") && !text.contains("..."), "{text}");
        assert_eq!(
            Command::decode(b"relaod\n").unwrap_err().to_string(),
            "unknown command 'relaod'; did you mean 'reload'? (reload, quit, toggle, status, trigger)"
        );
    }

    #[test]
    fn refusals() {
        let cases: Vec<(&[u8], IpcError)> = vec![
            (b"reload\r\n", IpcError::NotAscii),
            (b"reload", IpcError::NotAscii),
            ("reload\u{e9}\n".as_bytes(), IpcError::NotAscii),
            (b"\n", IpcError::Empty),
            (b"", IpcError::Empty),
            (b"quit now\n", IpcError::WrongArgs { command: "quit", expected: "" }),
            (b"toggle a b\n", IpcError::WrongArgs { command: "toggle", expected: "[bar]" }),
            (b"toggle Main\n", IpcError::BadBarName("Main".into())),
            (b"trigger\n", IpcError::WrongArgs { command: "trigger", expected: "<event> [json]" }),
            (b"trigger Bad\n", IpcError::BadEventName("Bad".into())),
            (b"dance\n", IpcError::UnknownCommand("dance".into())),
        ];
        for (bytes, want) in cases {
            assert_eq!(Command::decode(bytes), Err(want), "{bytes:?}");
        }
        assert!(matches!(Command::decode(b"toggle  main\n"), Err(IpcError::UnknownCommand(_))));
        assert!(matches!(Command::decode(b" reload\n"), Err(IpcError::UnknownCommand(_))));
        assert!(matches!(Command::decode(b"trigger x  {}\n"), Err(IpcError::UnknownCommand(_))));
        assert!(matches!(Command::decode(b"trigger x {\n"), Err(IpcError::BadJson(_))));
        let mut big = vec![b'a'; MAX_REQUEST_BYTES];
        big.push(b'\n');
        assert_eq!(
            Command::decode(&big),
            Err(IpcError::TooLong { len: MAX_REQUEST_BYTES + 1, limit: MAX_REQUEST_BYTES })
        );
    }

    #[test]
    fn error_text() {
        assert_eq!(IpcError::WrongArgs { command: "toggle", expected: "[bar]" }.to_string(), "usage: rsb toggle [bar]");
        assert_eq!(IpcError::WrongArgs { command: "quit", expected: "" }.to_string(), "usage: rsb quit");
        assert_eq!(
            IpcError::UnknownCommand("x".into()).to_string(),
            "unknown command 'x' (reload, quit, toggle, status, trigger)"
        );
        let long = IpcError::UnknownCommand("y".repeat(400)).to_string();
        assert!(long.len() < 120 && long.contains(&format!("{}...", "y".repeat(32))));
    }

    #[test]
    fn applied_settings_and_inspect_lines_have_one_wire_form() {
        assert_eq!(Command::Applied.encode(), "applied\n");
        assert_eq!(Command::decode(b"applied\n"), Ok(Command::Applied));
        assert_eq!(Command::Settings("{\"verb\":\"get\"}".into()).encode(), "settings {\"verb\":\"get\"}\n");
        assert_eq!(inspect(None, None, 0, 64, false, false).encode(), "inspect from=0\n");
        assert_eq!(
            inspect(Some("my/weather"), Some("main"), 0, 5, true, true).encode(),
            "inspect my/weather bar=main depth=5 style redact\n"
        );
        // A bare `inspect` is the first page of the summary.
        assert_eq!(Command::decode(b"inspect\n"), Ok(inspect(None, None, 0, 64, false, false)));
        // Keys and flags in any order after the first word.
        assert_eq!(
            Command::decode(b"inspect clock redact depth=2 bar=top style from=4\n"),
            Ok(inspect(Some("clock"), Some("top"), 4, 2, true, true))
        );
        assert_eq!(Command::decode(b"inspect bar=main style\n"), Ok(inspect(None, Some("main"), 0, 64, true, false)));
        // A widget may be called `style`: the first word is always the widget.
        assert_eq!(Command::decode(b"inspect style\n"), Ok(inspect(Some("style"), None, 0, 64, false, false)));
        assert!(Command::decode(b"inspect clock depth=64\n").is_ok(), "the deepest tree the scene allows");
    }

    #[test]
    fn a_settings_line_goes_through_from_args_with_non_ascii_escaped() {
        let request = "{\"changes\":[{\"op\":\"set\",\"key\":\"preset\",\"value\":\"caf\u{e9}\"}]}";
        let cmd = Command::from_args(&words(&["settings", request])).unwrap();
        let wire = cmd.encode();
        assert!(wire.is_ascii() && wire.contains("caf\\u00e9"), "{wire}");
        assert_eq!(Command::decode(wire.as_bytes()), Ok(cmd));
        assert!(matches!(Command::from_args(&words(&["settings", "{nope"])), Err(IpcError::BadJson(_))));
        assert!(matches!(
            Command::from_args(&words(&["settings"])),
            Err(IpcError::WrongArgs { command: "settings", .. })
        ));
        // The line is checked as a line; whether it is a request is for the settings reply to say.
        assert_eq!(Command::decode(b"settings {nope\n"), Ok(Command::Settings("{nope".into())));
        assert!(matches!(Command::decode(b"settings \n"), Err(IpcError::UnknownCommand(_))));
        assert!(matches!(Command::decode(b"settings  {}\n"), Err(IpcError::UnknownCommand(_))));
        assert_eq!(
            Command::decode(b"settings\n"),
            Err(IpcError::WrongArgs { command: "settings", expected: "<json>" })
        );
        assert_eq!(Command::decode("settings {\"a\":\"\u{e9}\"}\n".as_bytes()), Err(IpcError::NotAscii));
        let mut big = b"settings {\"a\":\"".to_vec();
        big.extend(vec![b'x'; MAX_REQUEST_BYTES]);
        big.extend(b"\"}\n");
        assert!(matches!(Command::decode(&big), Err(IpcError::TooLong { .. })));
    }

    #[test]
    fn inspect_refusals_name_the_word() {
        let arg = |arg: &str, why| IpcError::BadInspectArg { arg: arg.into(), why };
        let cases: Vec<(&[u8], IpcError)> = vec![
            (b"inspect Clock\n", IpcError::BadWidgetName("Clock".into())),
            (b"inspect ../x\n", IpcError::BadWidgetName("../x".into())),
            (b"inspect clock bar=Main\n", IpcError::BadBarName("Main".into())),
            (b"inspect clock depth=0\n", arg("depth=0", "depth is 1 to 64")),
            (b"inspect clock depth=65\n", arg("depth=65", "depth is 1 to 64")),
            (b"inspect clock depth=300\n", arg("depth=300", "depth is 1 to 64")),
            (b"inspect clock depth=-1\n", arg("depth=-1", "use a whole number")),
            (b"inspect clock depth=\n", arg("depth=", "use a whole number")),
            (b"inspect from=x\n", arg("from=x", "use a whole number")),
            (b"inspect from=99999999999\n", arg("from=99999999999", "use a whole number")),
            (b"inspect from=4294967296\n", arg("from=4294967296", "the number is too large")),
            (b"inspect clock wide\n", arg("wide", "unknown word")),
            (b"inspect clock colour=red\n", arg("colour=red", "unknown word")),
            (b"inspect clock style style\n", arg("style", "given twice")),
            (b"inspect clock bar=a bar=b\n", arg("bar=b", "given twice")),
            (b"inspect from=1 from=2\n", arg("from=2", "given twice")),
            ("inspect cl\u{f3}ck\n".as_bytes(), IpcError::NotAscii),
        ];
        for (bytes, want) in cases {
            assert_eq!(Command::decode(bytes), Err(want), "{}", String::from_utf8_lossy(bytes));
        }
        assert!(matches!(Command::decode(b"inspect  clock\n"), Err(IpcError::UnknownCommand(_))));
        assert_eq!(arg("depth=0", "depth is 1 to 64").to_string(), "bad inspect argument 'depth=0': depth is 1 to 64");
        assert!(IpcError::BadWidgetName("Clock".into()).to_string().contains("like clock or my/weather"));
    }

    #[test]
    fn replies() {
        for r in [Reply::Ok("{\"a\":1}".into()), Reply::Err("no".into()), Reply::Ok(String::new())] {
            assert_eq!(Reply::decode(r.encode().as_bytes()), Ok(r.clone()));
        }
        assert_eq!(Reply::Err("caf\u{e9}\r\nx".into()).encode(), "err caf???x\n");
        let wire = Reply::Err("x".repeat(MAX_REPLY_BYTES * 2)).encode();
        assert_eq!(wire.len(), MAX_REPLY_BYTES);
        assert!(Reply::decode(wire.as_bytes()).is_ok());
        // An answer too long to send is an error, not a document cut in the middle.
        let json = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_REPLY_BYTES));
        let wire = Reply::Ok(json).encode();
        assert!(wire.starts_with("err the answer is longer than"), "{wire}");
        assert!(matches!(Reply::decode(wire.as_bytes()), Ok(Reply::Err(_))));
        assert_eq!(Reply::decode(b"maybe\n"), Err(IpcError::BadReply));
        assert_eq!(Reply::decode(b"okay\n"), Err(IpcError::BadReply));
    }
}
