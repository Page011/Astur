//! Keys and values a change may name. Everything a caller sends is checked
//! here, and what is written into a file is rendered from the checked value,
//! never copied from the input: a quote, a bracket, a line break or a
//! semicolon cannot reach either file.

use serde_json::Value;

use crate::config::{BAR_KEYS, BAR_SPECS, BUILTIN_PRESETS, KeyKind, WidgetRef, bar_spec, is_valid_bar_name};
use crate::contract::{ErrorCode, SettingsError, Zone};
use crate::suggest;

/// The most widgets a zone list may name.
pub const MAX_ZONE_ITEMS: usize = 64;

/// A bar key a change may set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarKey {
    Position,
    Height,
    Floating,
    Zone(Zone),
}

impl BarKey {
    pub(crate) fn name(self) -> &'static str {
        match self {
            BarKey::Position => "position",
            BarKey::Height => "height",
            BarKey::Floating => "floating",
            BarKey::Zone(z) => z.name(),
        }
    }

    pub(crate) const ALL: [BarKey; 6] = [
        BarKey::Position,
        BarKey::Height,
        BarKey::Floating,
        BarKey::Zone(Zone::Left),
        BarKey::Zone(Zone::Center),
        BarKey::Zone(Zone::Right),
    ];
}

/// A key of a change, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Preset,
    Bar { bar: String, key: BarKey },
    Accent,
}

fn bad(message: impl Into<String>) -> SettingsError {
    SettingsError::new(ErrorCode::BadValue, message)
}

/// Text that is safe to show back to the person: short, printable ASCII, no
/// quotes. Anything else is described, never echoed.
pub(crate) fn shown(text: &str) -> String {
    let plain =
        text.len() <= 32 && text.bytes().all(|b| (0x20..0x7f).contains(&b) && b != b'\'' && b != b'"' && b != b'\\');
    if plain { format!("'{text}'") } else { "text with characters that are not allowed".to_string() }
}

/// A key the caller wrote: letters, digits, dots, `_` and `-` only.
fn key_chars_ok(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 96
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Parses `key`. `bar_names` are the names of the effective bars (for the
/// "did you mean" hint only).
pub(crate) fn parse_key(key: &str, bar_names: &[String]) -> Result<Key, SettingsError> {
    if !key_chars_ok(key) {
        return Err(bad(format!("{} is not a key; keys are like preset, bar.main.height or color.accent", shown(key)))
            .with_next("use a key from rsb settings schema"));
    }
    let parts: Vec<&str> = key.split('.').collect();
    let editable: Vec<&str> = BAR_SPECS.iter().filter(|s| s.editable).map(|s| s.name).collect();
    let unknown = || {
        let mut all: Vec<String> = vec!["preset".into(), "color.accent".into()];
        for b in bar_names {
            all.extend(editable.iter().map(|k| format!("bar.{b}.{k}")));
        }
        let hint = suggest::hint(key, all.iter().map(String::as_str));
        SettingsError::new(ErrorCode::UnknownKey, format!("unknown key {}{hint}", shown(key)))
            .with_next("use a key from rsb settings schema")
    };
    match parts.as_slice() {
        ["preset"] => Ok(Key::Preset),
        ["color", "accent"] => Ok(Key::Accent),
        ["color", other] if is_valid_bar_name(other) => Err(SettingsError::new(
            ErrorCode::FileOnly,
            format!("'color.{other}' is changed in style.css only; only color.accent is changed here"),
        )
        .with_next("write a rule for it in style.css; the colours are listed in docs\\authoring.md")),
        ["bar", bar, name] if is_valid_bar_name(bar) => {
            if let Some(k) = BarKey::ALL.iter().find(|k| k.name() == *name) {
                return Ok(Key::Bar { bar: (*bar).to_string(), key: *k });
            }
            if BAR_KEYS.contains(name) {
                let example = match bar_spec(name).kind {
                    KeyKind::Int { lo, .. } => format!("{name} = {lo}"),
                    KeyKind::Bool => format!("{name} = true"),
                    KeyKind::Choice(words) => format!("{name} = \"{}\"", words[0]),
                    KeyKind::Monitors { .. } => format!("{name} = \"primary\""),
                    KeyKind::Name | KeyKind::Text => format!("{name} = \"main\""),
                    KeyKind::Zone => format!("{name} = [\"clock\"]"),
                };
                return Err(SettingsError::new(
                    ErrorCode::FileOnly,
                    format!("'bar.{bar}.{name}' is changed in bar.toml only"),
                )
                .with_next(format!(
                    "add a line such as {example} under [[bar]] in bar.toml; the keys are listed under \"Bars\" in docs\\authoring.md"
                )));
            }
            Err(unknown())
        }
        _ => Err(unknown()),
    }
}

/// A bar name or index as a change writes it: the same characters a bar name
/// has (an index is digits).
pub(crate) fn bar_token(token: &str) -> Result<(), SettingsError> {
    if is_valid_bar_name(token) {
        Ok(())
    } else {
        Err(bad(format!("{} is not a bar name (a-z, 0-9, _ and -)", shown(token))))
    }
}

pub(crate) fn preset(key: &str, v: &Value) -> Result<&'static str, SettingsError> {
    let Some(name) = v.as_str() else {
        return Err(bad(format!("'{key}' must be a preset name")));
    };
    if let Some(p) = BUILTIN_PRESETS.iter().find(|p| **p == name) {
        return Ok(p);
    }
    let plausible =
        !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !plausible {
        return Err(bad(format!("{} is not a preset name", shown(name))).with_next("rsb settings presets lists them"));
    }
    // Names are lower case: 'Glass' is a preset that does not exist, and the hint says which one was meant.
    let lower = name.to_ascii_lowercase();
    let hint = if BUILTIN_PRESETS.contains(&lower.as_str()) {
        format!("; did you mean '{lower}'?")
    } else {
        suggest::hint(&lower, BUILTIN_PRESETS.iter().copied())
    };
    Err(SettingsError::new(ErrorCode::UnknownPreset, format!("no preset '{name}'{hint}"))
        .with_next("rsb settings presets lists them"))
}

/// `top` or `bottom`, in any case.
pub(crate) fn position(key: &str, v: &Value) -> Result<&'static str, SettingsError> {
    let words = match bar_spec("position").kind {
        KeyKind::Choice(w) => w,
        _ => &[],
    };
    let list = || words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(" or ");
    match v.as_str().and_then(|s| words.iter().find(|w| w.eq_ignore_ascii_case(s))) {
        Some(w) => Ok(w),
        None => Err(bad(format!("'{key}' must be {}", list()))),
    }
}

pub(crate) fn height(key: &str, v: &Value) -> Result<u32, SettingsError> {
    let (lo, hi) = match bar_spec("height").kind {
        KeyKind::Int { lo, hi } => (lo, hi),
        _ => (0, 0),
    };
    match v.as_i64() {
        Some(n) if (lo..=hi).contains(&n) => Ok(n as u32),
        Some(n) => Err(bad(format!("'{key}' must be {lo} to {hi}, got {n}"))),
        None => Err(bad(format!("'{key}' must be a whole number from {lo} to {hi}"))),
    }
}

pub(crate) fn floating(key: &str, v: &Value) -> Result<bool, SettingsError> {
    v.as_bool().ok_or_else(|| bad(format!("'{key}' must be true or false")))
}

/// One widget ref as a change writes it. The parser's own wording is shown
/// only when the text is safe to echo.
pub(crate) fn widget_ref(text: &str) -> Result<WidgetRef, SettingsError> {
    WidgetRef::parse(text).map_err(|why| {
        if text.len() <= 64 && text.bytes().all(|b| (0x20..0x7f).contains(&b) && !matches!(b, b'\'' | b'"' | b'\\')) {
            bad(why)
        } else {
            bad("a widget name uses a-z, 0-9, _ and -, with / between folders and one # before an instance name")
        }
    })
}

pub(crate) fn zone_list(key: &str, v: &Value) -> Result<Vec<WidgetRef>, SettingsError> {
    let Some(list) = v.as_array() else {
        return Err(bad(format!("'{key}' must be a list of widget names")));
    };
    if list.len() > MAX_ZONE_ITEMS {
        return Err(bad(format!("'{key}' holds {} widgets; at most {MAX_ZONE_ITEMS}", list.len())));
    }
    let mut out = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(bad(format!("entry {} of '{key}' must be a widget name", i + 1)));
        };
        out.push(widget_ref(text).map_err(|e| bad(format!("entry {} of '{key}': {}", i + 1, e.message)))?);
    }
    Ok(out)
}

/// `#rgb` or `#rrggbb`, hex digits only; returned as `#rrggbb` in lower case.
pub(crate) fn accent(key: &str, v: &Value) -> Result<String, SettingsError> {
    let fail = || bad(format!("'{key}' must be a colour like #e5a50a or #fa0"));
    let text = v.as_str().ok_or_else(fail)?;
    let hex = text.strip_prefix('#').ok_or_else(fail)?;
    if !(hex.len() == 3 || hex.len() == 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail());
    }
    let hex = hex.to_ascii_lowercase();
    Ok(if hex.len() == 3 {
        let mut s = String::from("#");
        for c in hex.chars() {
            s.push(c);
            s.push(c);
        }
        s
    } else {
        format!("#{hex}")
    })
}

/// The quoted TOML text of an already validated token.
pub(crate) fn quoted(token: &str) -> String {
    debug_assert!(token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'/' | b'#')));
    format!("\"{token}\"")
}

/// `["a", "b"]` from validated refs.
pub(crate) fn zone_text<'a>(items: impl IntoIterator<Item = &'a String>) -> String {
    let parts: Vec<String> = items.into_iter().map(|i| quoted(i)).collect();
    format!("[{}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_told_apart() {
        let names = vec!["main".to_string()];
        assert_eq!(parse_key("preset", &names), Ok(Key::Preset));
        assert_eq!(parse_key("color.accent", &names), Ok(Key::Accent));
        assert_eq!(
            parse_key("bar.main.right", &names),
            Ok(Key::Bar { bar: "main".into(), key: BarKey::Zone(Zone::Right) })
        );
        assert_eq!(parse_key("bar.main.margin", &names).unwrap_err().code, ErrorCode::FileOnly);
        assert_eq!(parse_key("color.fg", &names).unwrap_err().code, ErrorCode::FileOnly);
        let e = parse_key("bar.main.hieght", &names).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnknownKey);
        assert!(e.message.contains("did you mean 'bar.main.height'"), "{}", e.message);
        assert_eq!(parse_key("bar.main\"\n[[bar]].height", &names).unwrap_err().code, ErrorCode::BadValue);
        assert_eq!(parse_key("a;b", &names).unwrap_err().code, ErrorCode::BadValue);
    }

    #[test]
    fn values_are_checked_and_rendered_from_the_checked_value() {
        assert_eq!(position("k", &json!("BOTTOM")), Ok("bottom"));
        assert!(position("k", &json!("left")).is_err());
        assert_eq!(height("k", &json!(16)), Ok(16));
        assert_eq!(height("k", &json!(128)), Ok(128));
        assert!(height("k", &json!(15)).unwrap_err().message.contains("16 to 128, got 15"));
        assert!(
            height("k", &json!(129)).is_err()
                && height("k", &json!(36.5)).is_err()
                && height("k", &json!("36")).is_err()
        );
        assert_eq!(accent("k", &json!("#FA0")), Ok("#ffaa00".into()));
        assert_eq!(accent("k", &json!("#E5A50A")), Ok("#e5a50a".into()));
        for bad in ["#12", "#12345", "#1234567", "e5a50a", "#e5a50g", "#fff */ x", "red", "#fff;"] {
            assert_eq!(accent("k", &json!(bad)).unwrap_err().code, ErrorCode::BadValue, "{bad}");
        }
        assert_eq!(preset("k", &json!("glass")), Ok("glass"));
        assert_eq!(preset("k", &json!("glas")).unwrap_err().code, ErrorCode::UnknownPreset);
        assert_eq!(preset("k", &json!("glass\"\n[[bar]]")).unwrap_err().code, ErrorCode::BadValue);
        assert!(zone_list("k", &json!(["clock", "clock#utc", "my/weather"])).is_ok());
        assert!(zone_list("k", &json!(vec!["clock"; 65])).is_err());
        assert!(zone_list("k", &json!(["clock", 3])).is_err());
        assert_eq!(zone_list("k", &json!(["a\"b"])).unwrap_err().code, ErrorCode::BadValue);
    }

    #[test]
    fn a_message_never_echoes_text_that_is_not_plain() {
        let e = preset("k", &json!("x\u{202e}\ny")).unwrap_err();
        assert!(!e.message.contains('\n') && !e.message.contains('\u{202e}'), "{}", e.message);
        assert_eq!(shown("ok-1"), "'ok-1'");
        assert!(shown("a'b").starts_with("text with"));
    }
}
