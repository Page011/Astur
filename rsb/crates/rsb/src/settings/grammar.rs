//! The words of `rsb settings`: every verb and its options turned into a
//! request, with no I/O (a file given to `change` is read by the caller).
//! A mistake is a `bad_usage` refusal that carries the verb's usage line,
//! so the command line answers it with the same JSON as any other refusal.

use std::path::PathBuf;
use std::str::FromStr;

use rsb_core::config::WidgetRef;
use rsb_core::config::is_valid_bar_name;
use rsb_core::contract::{
    ConfigFile, ErrorCode, Expect, FileHash, FromRef, Op, Request, SettingsError, Target, Verb, Zone,
};
use rsb_core::preset::Scheme;
use serde_json::Value;

/// The verbs in the order the help lists them; "did you mean" reads this.
pub const VERBS: &[&str] = &[
    "get", "presets", "preview", "widgets", "schema", "set", "unset", "move", "add", "remove", "change", "undo",
    "restore", "log",
];

/// Widest and narrowest picture a preview may ask for, logical px.
pub const PREVIEW_WIDTH: std::ops::RangeInclusive<u32> = 200..=7680;
/// Smallest and largest preview scale.
pub const PREVIEW_SCALE: std::ops::RangeInclusive<f32> = 0.5..=4.0;

/// What `rsb settings ...` asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsCmd {
    pub call: Call,
    /// `--no-wait`: do not ask the bar whether it shows the change.
    pub no_wait: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// A request the verbs built.
    Request(Request),
    /// `change <file | ->`: the request is in the file (None: standard
    /// input); `flags` holds what the options gave and wins over the file.
    Change {
        from: Option<PathBuf>,
        flags: Request,
    },
    Preview(PreviewArgs),
    /// The words were wrong.
    Refused(SettingsError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewArgs {
    pub presets: Vec<String>,
    pub all: bool,
    pub themes: Vec<Scheme>,
    pub width: u32,
    pub scale: f32,
    pub mine: bool,
    /// With `mine`: the zones come from the preset, everything else from the user's files.
    pub zones_from_preset: bool,
    pub cache_dir: Option<PathBuf>,
}

/// The usage line of one verb.
pub fn usage_of(verb: &str) -> &'static str {
    match verb {
        "get" => "usage: rsb settings get [--bar NAME]",
        "presets" => "usage: rsb settings presets",
        "preview" => {
            "usage: rsb settings preview <preset>|--all [--theme dark|light|both] [--width N] [--scale S] [--mine [--zones mine|preset]] [--cache-dir DIR]"
        }
        "widgets" => "usage: rsb settings widgets",
        "schema" => "usage: rsb settings schema",
        "set" => "usage: rsb settings set <key> <value>",
        "unset" => "usage: rsb settings unset <key>",
        "move" => "usage: rsb settings move <widget> <bar>.<zone>[:<index>] [--from BAR[.ZONE[:INDEX]]]",
        "add" => "usage: rsb settings add <widget> <bar>.<zone>[:<index>]",
        "remove" => "usage: rsb settings remove <widget> [--from BAR[.ZONE[:INDEX]]]",
        "change" => "usage: rsb settings change <file.json | ->",
        "undo" => "usage: rsb settings undo [N]",
        "restore" => "usage: rsb settings restore <bar.toml|style.css>",
        "log" => "usage: rsb settings log [--last N]",
        _ => {
            "usage: rsb settings <get|presets|preview|widgets|schema|set|unset|move|add|remove|change|undo|restore|log>"
        }
    }
}

/// A message with every character that is not printable ASCII written out as
/// `\u{..}`: a word typed on the command line never reaches a terminal raw.
fn plain_text(text: &str) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c.to_string() } else { c.escape_unicode().to_string() })
        .collect()
}

fn refuse(verb: &str, message: impl Into<String>) -> SettingsCmd {
    let error = SettingsError::new(ErrorCode::BadUsage, plain_text(&message.into())).with_next(usage_of(verb));
    SettingsCmd { call: Call::Refused(error), no_wait: false }
}

/// Options and positional words of one verb.
#[derive(Default)]
struct Words {
    positional: Vec<String>,
    contract: Option<u32>,
    expect: Expect,
    dry_run: bool,
    no_wait: bool,
    reset: bool,
    bar: Option<String>,
    from: Option<String>,
    last: Option<u32>,
    cache_dir: Option<PathBuf>,
    theme: Option<String>,
    width: Option<u32>,
    scale: Option<f32>,
    mine: bool,
    zones: Option<String>,
    all: bool,
}

const WRITING: &[&str] = &["set", "unset", "move", "add", "remove", "change", "undo", "restore"];

fn collect(verb: &str, words: &[String]) -> Result<Words, String> {
    let mut w = Words::default();
    let mut it = words.iter();
    let writes = WRITING.contains(&verb);
    while let Some(word) = it.next() {
        if !word.starts_with("--") {
            w.positional.push(word.clone());
            continue;
        }
        let allowed =
            |ok: bool| if ok { Ok(()) } else { Err(format!("'{word}' does not apply to rsb settings {verb}")) };
        let mut value = || it.next().cloned().ok_or_else(|| format!("{word} needs a value"));
        match word.as_str() {
            "--contract" => {
                let v = value()?;
                w.contract = Some(v.parse().map_err(|_| format!("--contract: '{v}' is not a version number"))?);
            }
            "--expect" => {
                allowed(writes)?;
                let v = value()?;
                let (name, hash) =
                    v.split_once('=').ok_or_else(|| format!("--expect: '{v}' is not <file>=<version>"))?;
                let file = ConfigFile::from_str(name)
                    .map_err(|()| format!("--expect: '{name}' is not bar.toml or style.css"))?;
                w.expect.set(file, FileHash::from_str(hash).map_err(|e| format!("--expect: {e}"))?);
            }
            "--dry-run" => {
                allowed(writes)?;
                w.dry_run = true;
            }
            "--no-wait" => {
                allowed(writes)?;
                w.no_wait = true;
            }
            "--reset" => {
                allowed(verb == "set")?;
                w.reset = true;
            }
            "--bar" => {
                allowed(verb == "get")?;
                w.bar = Some(value()?);
            }
            "--from" => {
                allowed(matches!(verb, "move" | "remove"))?;
                w.from = Some(value()?);
            }
            "--last" => {
                allowed(verb == "log")?;
                let v = value()?;
                w.last = Some(v.parse().map_err(|_| format!("--last: '{v}' is not a number"))?);
            }
            "--cache-dir" => {
                allowed(verb == "preview")?;
                w.cache_dir = Some(PathBuf::from(value()?));
            }
            "--theme" => {
                allowed(verb == "preview")?;
                w.theme = Some(value()?);
            }
            "--width" => {
                allowed(verb == "preview")?;
                let v = value()?;
                w.width = Some(v.parse().map_err(|_| format!("--width: '{v}' is not a number"))?);
            }
            "--scale" => {
                allowed(verb == "preview")?;
                let v = value()?;
                w.scale = Some(v.parse().map_err(|_| format!("--scale: '{v}' is not a number"))?);
            }
            "--mine" => {
                allowed(verb == "preview")?;
                w.mine = true;
            }
            "--zones" => {
                allowed(verb == "preview")?;
                w.zones = Some(value()?);
            }
            "--all" => {
                allowed(verb == "preview")?;
                w.all = true;
            }
            other => return Err(format!("unknown option '{other}' for rsb settings {verb}")),
        }
    }
    Ok(w)
}

/// `<bar>.<zone>[:<index>]`.
fn target(text: &str) -> Result<Target, String> {
    let (place, index) = match text.rsplit_once(':') {
        Some((p, i)) => {
            let n: i32 =
                i.parse().map_err(|_| format!("'{i}' in '{text}' is not an index (0, 1, ... or -1 for the end)"))?;
            (p, Some(n))
        }
        None => (text, None),
    };
    let (bar, zone) =
        place.split_once('.').ok_or_else(|| format!("'{text}' is not <bar>.<zone> (zones: left, center, right)"))?;
    if !is_valid_bar_name(bar) {
        return Err(format!("'{bar}' is not a bar name or index"));
    }
    let zone = Zone::ALL
        .into_iter()
        .find(|z| z.name() == zone)
        .ok_or_else(|| format!("'{zone}' is not a zone (left, center, right)"))?;
    Ok(Target { bar: bar.to_string(), zone, index })
}

fn widget(text: &str) -> Result<String, String> {
    WidgetRef::parse(text).map(|_| text.to_string())
}

/// `--from`: a bar, or the exact place `<bar>.<zone>[:<index>]`.
fn from_ref(text: &str) -> Result<FromRef, String> {
    if text.contains('.') { target(text).map(FromRef::At) } else { bar_name(text).map(FromRef::Bar) }
}

fn bar_name(text: &str) -> Result<String, String> {
    if is_valid_bar_name(text) { Ok(text.to_string()) } else { Err(format!("'{text}' is not a bar name or index")) }
}

/// The value of `set`: JSON when it parses (`40`, `true`, a list), else the
/// word itself.
fn value_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

fn request(verb: Verb, w: &Words) -> Request {
    Request {
        contract: w.contract,
        verb: Some(verb),
        expect: w.expect.clone(),
        dry_run: w.dry_run,
        reset: w.reset,
        ..Request::default()
    }
}

/// Parses the words after `settings`.
pub fn parse(words: &[String]) -> SettingsCmd {
    let Some(verb) = words.first() else {
        return refuse("", "rsb settings needs a verb");
    };
    let rest = &words[1..];
    if !VERBS.contains(&verb.as_str()) {
        let hint = rsb_core::suggest::hint(verb, VERBS.iter().copied());
        return refuse("", format!("unknown settings command '{verb}'{hint}"));
    }
    let w = match collect(verb, rest) {
        Ok(w) => w,
        Err(e) => return refuse(verb, e),
    };
    match build(verb, w) {
        Ok(cmd) => cmd,
        Err(e) => refuse(verb, e),
    }
}

fn exactly(verb: &str, w: &Words, n: usize) -> Result<(), String> {
    if w.positional.len() == n {
        Ok(())
    } else {
        Err(format!("rsb settings {verb} takes {n} word{}, got {}", if n == 1 { "" } else { "s" }, w.positional.len()))
    }
}

fn build(verb: &str, w: Words) -> Result<SettingsCmd, String> {
    let done = |call| Ok(SettingsCmd { call, no_wait: w.no_wait });
    let p = &w.positional;
    match verb {
        "get" => {
            exactly(verb, &w, 0)?;
            let mut r = request(Verb::Get, &w);
            r.bar = w.bar.as_deref().map(bar_name).transpose()?;
            done(Call::Request(r))
        }
        "presets" | "widgets" | "schema" => {
            exactly(verb, &w, 0)?;
            let v = match verb {
                "presets" => Verb::Presets,
                "widgets" => Verb::Widgets,
                _ => Verb::Schema,
            };
            done(Call::Request(request(v, &w)))
        }
        "log" => {
            exactly(verb, &w, 0)?;
            let mut r = request(Verb::Log, &w);
            r.last = w.last;
            done(Call::Request(r))
        }
        "set" => {
            exactly(verb, &w, 2)?;
            let mut r = request(Verb::Change, &w);
            r.changes.push(Op::Set { key: p[0].clone(), value: value_of(&p[1]) });
            done(Call::Request(r))
        }
        "unset" => {
            exactly(verb, &w, 1)?;
            let mut r = request(Verb::Change, &w);
            r.changes.push(Op::Unset { key: p[0].clone() });
            done(Call::Request(r))
        }
        "move" | "add" => {
            exactly(verb, &w, 2)?;
            let (widget, to) = (widget(&p[0])?, target(&p[1])?);
            let mut r = request(Verb::Change, &w);
            r.changes.push(if verb == "move" {
                Op::Move { widget, from: w.from.as_deref().map(from_ref).transpose()?, to }
            } else {
                Op::Add { widget, to }
            });
            done(Call::Request(r))
        }
        "remove" => {
            exactly(verb, &w, 1)?;
            let mut r = request(Verb::Change, &w);
            r.changes.push(Op::Remove { widget: widget(&p[0])?, from: w.from.as_deref().map(from_ref).transpose()? });
            done(Call::Request(r))
        }
        "change" => {
            exactly(verb, &w, 1)?;
            let from = (p[0] != "-").then(|| PathBuf::from(&p[0]));
            done(Call::Change { from, flags: request(Verb::Change, &w) })
        }
        "undo" => {
            if p.len() > 1 {
                return Err("rsb settings undo takes at most one number".into());
            }
            let mut r = request(Verb::Undo, &w);
            r.n = p.first().map(|n| n.parse().map_err(|_| format!("'{n}' is not a change number"))).transpose()?;
            done(Call::Request(r))
        }
        "restore" => {
            exactly(verb, &w, 1)?;
            let mut r = request(Verb::Restore, &w);
            r.file =
                Some(ConfigFile::from_str(&p[0]).map_err(|()| format!("'{}' is not bar.toml or style.css", p[0]))?);
            done(Call::Request(r))
        }
        "preview" => {
            if w.all == !p.is_empty() {
                return Err("name one preset, or use --all".into());
            }
            if p.len() > 1 {
                return Err("name one preset, or use --all".into());
            }
            let themes = match w.theme.as_deref() {
                None | Some("both") => Scheme::ALL.to_vec(),
                Some(name) => vec![
                    Scheme::from_name(name).ok_or_else(|| format!("--theme: '{name}' is not dark, light or both"))?,
                ],
            };
            let zones_from_preset = match w.zones.as_deref() {
                None | Some("mine") => false,
                Some("preset") => true,
                Some(other) => return Err(format!("--zones: '{other}' is not mine or preset")),
            };
            if w.zones.is_some() && !w.mine {
                return Err("--zones goes with --mine".into());
            }
            let width = w.width.unwrap_or(1920);
            let scale = w.scale.unwrap_or(1.0);
            if !PREVIEW_WIDTH.contains(&width) {
                return Err(format!("--width must be {} to {}", PREVIEW_WIDTH.start(), PREVIEW_WIDTH.end()));
            }
            if !PREVIEW_SCALE.contains(&scale) {
                return Err("--scale must be 0.5 to 4".into());
            }
            done(Call::Preview(PreviewArgs {
                presets: p.clone(),
                all: w.all,
                themes,
                width,
                scale,
                mine: w.mine,
                zones_from_preset,
                cache_dir: w.cache_dir.clone(),
            }))
        }
        _ => Err(format!("unknown settings command '{verb}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    fn request_of(line: &str) -> Request {
        match parse(&words(line)).call {
            Call::Request(r) => r,
            other => panic!("not a request: {other:?}"),
        }
    }

    fn refusal(line: &str) -> SettingsError {
        match parse(&words(line)).call {
            Call::Refused(e) => e,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn every_verb_parses_into_its_request() {
        assert_eq!(request_of("get").verb, Some(Verb::Get));
        assert_eq!(request_of("get --bar main").bar.as_deref(), Some("main"));
        assert_eq!(request_of("presets").verb, Some(Verb::Presets));
        assert_eq!(request_of("widgets").verb, Some(Verb::Widgets));
        assert_eq!(request_of("schema").verb, Some(Verb::Schema));
        assert_eq!(request_of("log").last, None);
        assert_eq!(request_of("log --last 3").last, Some(3));
        assert_eq!(request_of("undo").n, None);
        assert_eq!(request_of("undo 17").n, Some(17));
        assert_eq!(request_of("restore bar.toml").file, Some(ConfigFile::BarToml));
        assert_eq!(request_of("restore style.css").file, Some(ConfigFile::StyleCss));
        let set = request_of("set bar.main.height 40");
        assert_eq!(set.verb, Some(Verb::Change));
        assert_eq!(set.changes, [Op::Set { key: "bar.main.height".into(), value: Value::from(40) }]);
        assert_eq!(
            request_of("set preset glass").changes,
            [Op::Set { key: "preset".into(), value: Value::from("glass") }]
        );
        assert_eq!(
            request_of("set color.accent #e5a50a").changes,
            [Op::Set { key: "color.accent".into(), value: Value::from("#e5a50a") }]
        );
        assert_eq!(
            request_of("set bar.main.floating true").changes,
            [Op::Set { key: "bar.main.floating".into(), value: Value::Bool(true) }]
        );
        assert_eq!(
            request_of(r#"set bar.main.right ["status","clock"]"#).changes,
            [Op::Set { key: "bar.main.right".into(), value: serde_json::json!(["status", "clock"]) }]
        );
        assert_eq!(request_of("unset bar.main.height").changes, [Op::Unset { key: "bar.main.height".into() }]);
    }

    #[test]
    fn move_add_and_remove_read_their_targets() {
        assert_eq!(
            request_of("move clock main.right:0 --from main").changes,
            [Op::Move {
                widget: "clock".into(),
                from: Some(FromRef::Bar("main".into())),
                to: Target { bar: "main".into(), zone: Zone::Right, index: Some(0) },
            }]
        );
        assert_eq!(
            request_of("move clock 0.left").changes,
            [Op::Move {
                widget: "clock".into(),
                from: None,
                to: Target { bar: "0".into(), zone: Zone::Left, index: None },
            }]
        );
        assert_eq!(
            request_of("add clock#utc main.center:-1").changes,
            [Op::Add {
                widget: "clock#utc".into(),
                to: Target { bar: "main".into(), zone: Zone::Center, index: Some(-1) },
            }]
        );
        assert_eq!(
            request_of("remove media --from main").changes,
            [Op::Remove { widget: "media".into(), from: Some(FromRef::Bar("main".into())) }]
        );
        assert_eq!(request_of("remove media").changes, [Op::Remove { widget: "media".into(), from: None }]);
    }

    #[test]
    fn writing_options_reach_the_request() {
        let r = request_of(
            "set preset glass --dry-run --reset --contract 1 --expect bar.toml=fnv1a64:3f0c9a1e5b7d2c44 --expect style.css=none",
        );
        assert!(r.dry_run && r.reset);
        assert_eq!(r.contract, Some(1));
        assert_eq!(r.expect.get(ConfigFile::BarToml), Some(FileHash::Fnv(0x3f0c9a1e5b7d2c44)));
        assert_eq!(r.expect.get(ConfigFile::StyleCss), Some(FileHash::Missing));
        assert!(parse(&words("set preset glass --no-wait")).no_wait);
        assert!(!parse(&words("set preset glass")).no_wait);
    }

    #[test]
    fn change_reads_a_file_or_standard_input() {
        match parse(&words("change plan.json --dry-run")).call {
            Call::Change { from, flags } => {
                assert_eq!(from, Some(PathBuf::from("plan.json")));
                assert!(flags.dry_run);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse(&words("change -")).call, Call::Change { from: None, .. }));
    }

    #[test]
    fn preview_takes_a_preset_or_all_with_defaults() {
        let Call::Preview(p) = parse(&words("preview glass")).call else { panic!() };
        assert_eq!(
            (p.presets.as_slice(), p.all, p.width, p.scale, p.mine),
            (&["glass".to_string()][..], false, 1920, 1.0, false)
        );
        assert_eq!(p.themes, Scheme::ALL.to_vec());
        let Call::Preview(p) =
            parse(&words("preview --all --theme light --width 800 --scale 2 --mine --cache-dir c")).call
        else {
            panic!()
        };
        assert_eq!((p.all, p.themes.clone(), p.width, p.scale, p.mine), (true, vec![Scheme::Light], 800, 2.0, true));
        assert_eq!(p.cache_dir, Some(PathBuf::from("c")));
        for bad in [
            "preview",
            "preview --all glass",
            "preview glass --theme blue",
            "preview glass --width 10",
            "preview glass --scale 9",
            "preview a b",
        ] {
            assert_eq!(refusal(bad).code, ErrorCode::BadUsage, "{bad}");
        }
    }

    #[test]
    fn a_mistake_is_bad_usage_with_the_usage_line() {
        for (line, verb) in [
            ("get now", "get"),
            ("set preset", "set"),
            ("set a b c", "set"),
            ("unset", "unset"),
            ("move clock", "move"),
            ("move clock main", "move"),
            ("move clock main.middle", "move"),
            ("move clock main.left:x", "move"),
            ("move Bad main.left", "move"),
            ("add clock", "add"),
            ("remove", "remove"),
            ("undo x", "undo"),
            ("undo 1 2", "undo"),
            ("restore other.txt", "restore"),
            ("log --last x", "log"),
            ("get --expect bar.toml=none", "get"),
            ("get --dry-run", "get"),
            ("presets --bogus", "presets"),
            ("set preset glass --expect bar.toml=sha:1", "set"),
            ("set preset glass --expect other=none", "set"),
            ("set preset glass --contract x", "set"),
            ("get --bar", "get"),
            ("unset a --reset", "unset"),
        ] {
            let e = refusal(line);
            assert_eq!(e.code, ErrorCode::BadUsage, "{line}: {e}");
            assert_eq!(e.next.as_deref(), Some(usage_of(verb)), "{line}");
            assert_eq!(e.exit_code(), 2);
        }
    }

    #[test]
    fn an_unknown_verb_gets_a_suggestion() {
        let e = refusal("gett");
        assert!(e.message.contains("unknown settings command 'gett'; did you mean 'get'?"), "{e}");
        let e = refusal("presetz");
        assert!(e.message.contains("did you mean 'presets'?"), "{e}");
        assert!(refusal("zzzzzz").message.ends_with("'zzzzzz'"));
        assert_eq!(parse(&[]).call, Call::Refused(refuse("", "rsb settings needs a verb").call.into_refusal()));
    }

    impl Call {
        fn into_refusal(self) -> SettingsError {
            match self {
                Call::Refused(e) => e,
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn every_verb_has_its_own_usage_line() {
        for verb in VERBS {
            assert!(usage_of(verb).starts_with(&format!("usage: rsb settings {verb}")), "{verb}");
        }
    }

    #[test]
    fn from_names_a_bar_or_an_exact_place() {
        let op = |line: &str| request_of(line).changes[0].clone();
        let from_of = |op: Op| match op {
            Op::Remove { from, .. } | Op::Move { from, .. } => from,
            other => panic!("{other:?}"),
        };
        assert_eq!(from_of(op("remove clock --from main")), Some(FromRef::Bar("main".into())));
        let at = |zone, index| Some(FromRef::At(Target { bar: "main".into(), zone, index }));
        assert_eq!(from_of(op("remove clock --from main.left")), at(Zone::Left, None));
        assert_eq!(from_of(op("move clock main.right:0 --from main.center:1")), at(Zone::Center, Some(1)));
        assert!(refusal("remove clock --from main.top").message.contains("is not a zone"));
    }

    #[test]
    fn zones_choose_which_layout_a_mine_preview_draws() {
        let args = |line: &str| match parse(&words(line)).call {
            Call::Preview(p) => p,
            other => panic!("{other:?}"),
        };
        assert!(!args("preview glass --mine").zones_from_preset);
        assert!(args("preview glass --mine --zones preset").zones_from_preset);
        assert!(!args("preview glass --mine --zones mine").zones_from_preset);
        assert!(refusal("preview glass --mine --zones other").message.contains("not mine or preset"));
        assert!(refusal("preview glass --zones preset").message.contains("goes with --mine"));
    }

    #[test]
    fn a_word_with_control_characters_is_shown_escaped() {
        let e = refusal("get --bar x\u{1b}[2J\u{7}");
        assert!(!e.message.contains('\u{1b}') && !e.message.contains('\u{7}'), "{:?}", e.message);
        assert!(e.message.contains(r"\u{1b}"), "{}", e.message);
    }
}
