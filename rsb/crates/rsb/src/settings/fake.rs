//! A backend that returns what the real modules would, so the reply logic
//! is tested without a bar, a config folder or the other modules.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;

use rsb_core::cfgedit::{FilePlan, FileTexts, Plan};
use rsb_core::config::{ConfigSources, Sourced, load_config};
use rsb_core::contract::{
    AppliedReply, ConfigFile, FileHash, KeptKey, Lowest, Notice, NoticeCode, Op, PresetContrast, PresetSwitch,
    SchemeContrast, SettingsError, Source,
};
use rsb_core::diag::SourcePos;
use rsb_core::preset::Scheme;
use serde_json::json;

use super::backend::*;
use crate::cfgwrite::{Applied, FileOutcome, LogEntry, WriteOpts, Written};
use crate::preview::{Preview, PreviewOpts};
use crate::settings::Ctx;

pub const BAR_HASH: FileHash = FileHash::Fnv(0x3f0c9a1e5b7d2c44);
pub const CSS_HASH: FileHash = FileHash::Fnv(0x91aa04c3e2b8f610);
pub const NEW_BAR_HASH: FileHash = FileHash::Fnv(0x77d1f0e4a2c95b38);

#[derive(Default)]
pub struct Fake {
    pub loaded: RefCell<Loaded>,
    pub plan: RefCell<Option<Result<Plan, SettingsError>>>,
    pub applied: RefCell<Option<Result<Applied, SettingsError>>>,
    pub log_entries: RefCell<Vec<LogEntry>>,
    pub written: Cell<Written>,
    pub last_good: Cell<bool>,
    pub presets: RefCell<Vec<PresetView>>,
    pub listing: RefCell<Listing>,
    pub previews: RefCell<Vec<Preview>>,
    pub preview_error: RefCell<Option<SettingsError>>,
    /// What `applied` answers, one per ask; the last repeats.
    pub bar: RefCell<VecDeque<BarState>>,
    /// What `bar_loads` answers (the bar's load count before the write); none when a test does not care.
    pub loads_before: Cell<Option<u64>>,
    pub high_contrast: Cell<bool>,
    pub clock: Cell<u64>,
    pub sleeps: RefCell<Vec<u64>>,
    /// Lines `append_log` got.
    pub log: RefCell<Vec<String>>,
    /// The names of the calls made, in order.
    pub calls: RefCell<Vec<String>>,
    /// The options the writer was given.
    pub opts: RefCell<Vec<WriteOpts>>,
}

impl Fake {
    fn call(&self, name: &str) {
        self.calls.borrow_mut().push(name.to_string());
    }

    pub fn called(&self, name: &str) -> usize {
        self.calls.borrow().iter().filter(|c| *c == name).count()
    }
}

/// The default config with every value from the user's file at line 2 (so
/// `source` is `user`), except where a test says otherwise.
pub fn sources() -> ConfigSources {
    let config = load_config(None, &|_| None).config.expect("the defaults load");
    let mut s = ConfigSources::all_default(&config);
    s.preset = Sourced { value: "fluent".into(), source: Source::User, at: Some(SourcePos::new("bar.toml", 1, 1)) };
    let bar = &mut s.bars[0];
    bar.height = Sourced { value: 40, source: Source::User, at: Some(SourcePos::new("bar.toml", 4, 1)) };
    bar.position.source = Source::Preset;
    s
}

pub fn accent(value: &str, source: Source, at: Option<&str>) -> SchemeAccent {
    SchemeAccent {
        value: Some(value.into()),
        source,
        at: at.map(str::to_string),
        shadowed_by: None,
        overridden: Vec::new(),
    }
}

/// A folder in good order: both files read, config in effect.
pub fn loaded() -> Loaded {
    Loaded {
        bar_toml: FileRead {
            hash: Some(BAR_HASH),
            text: Some("preset = \"fluent\"\n".into()),
            utf8: true,
            unreadable: None,
        },
        style_css: FileRead { hash: Some(CSS_HASH), text: Some("/* mine */\n".into()), utf8: true, unreadable: None },
        sources: Some(sources()),
        bar_error: None,
        style_error: None,
        accent: Some(AccentView {
            dark: accent("#e5a50a", Source::User, Some("style.css:42:5")),
            light: SchemeAccent {
                shadowed_by: Some("style.css:50:3".into()),
                overridden: vec!["style.css:12:5".into()],
                ..accent("#e5a50a", Source::User, Some("style.css:45:5"))
            },
        }),
    }
}

pub fn outcome(file: ConfigFile, hash: FileHash, written: bool, edited: bool) -> FileOutcome {
    FileOutcome { file, hash, written, edited_by_hand: edited }
}

/// A change that wrote `bar.toml` only.
pub fn bar_written() -> Applied {
    Applied {
        files: vec![
            outcome(ConfigFile::BarToml, NEW_BAR_HASH, true, false),
            outcome(ConfigFile::StyleCss, CSS_HASH, false, true),
        ],
        undo: Some(17),
        notices: vec![
            Notice::new(
                NoticeCode::HandEditKept,
                "bar.toml was edited by hand since the last settings change; that edit is kept",
            )
            .with_file(ConfigFile::BarToml),
        ],
        kept_copy: None,
    }
}

pub fn plan_of(what: &str) -> Plan {
    Plan {
        files: vec![FilePlan {
            file: ConfigFile::BarToml,
            base: BAR_HASH,
            created: false,
            bom: false,
            splices: Vec::new(),
            after: String::new(),
            what: what.into(),
        }],
        changes: vec![Op::Set { key: "preset".into(), value: json!("glass") }],
        notices: Vec::new(),
        preset_switch: None,
        accent: None,
    }
}

pub fn preset_switch() -> PresetSwitch {
    PresetSwitch {
        from: "default".into(),
        to: "glass".into(),
        kept: vec![KeptKey {
            bar: "main".into(),
            key: "floating".into(),
            yours: json!(false),
            preset: json!(true),
            same: false,
        }],
        bars_listed: 1,
        preset_bars: 1,
        accent_kept: Some("#e5a50a".into()),
    }
}

pub fn applied_reply(dir: &str, bar: FileHash, css: FileHash) -> AppliedReply {
    AppliedReply {
        bar_toml: Some(bar),
        style_css: Some(css),
        diagnostics: 0,
        loads: 1,
        config_dir: dir.into(),
        log_dir: dir.into(),
    }
}

pub fn preset_view(name: &str) -> PresetView {
    let scheme = |text: f64, graphic: f64| SchemeContrast {
        pass: true,
        text: Lowest { lowest: text, what: "placeholder on input".into(), need: 4.5 },
        graphic: Lowest { lowest: graphic, what: "dim on row".into(), need: 3.0 },
    };
    PresetView {
        name: name.into(),
        title: "Default".into(),
        description: "Chevron segments along the top edge.".into(),
        schemes: vec![Scheme::Dark, Scheme::Light],
        bars: 1,
        position: "top".into(),
        height: 36,
        floating: false,
        tune: vec![TuneView {
            var: "accent".into(),
            label: "Accent".into(),
            editable: true,
            dark: Some("#81cbea".into()),
            light: Some("#81cbea".into()),
        }],
        contrast: PresetContrast { dark: Some(scheme(4.67, 3.02)), light: Some(scheme(4.59, 3.46)) },
    }
}

impl Backend for Fake {
    fn load(&self, _dir: &Path, _with_accent: bool) -> Loaded {
        self.call("load");
        self.loaded.borrow().clone()
    }

    fn plan(
        &self,
        _dir: &Path,
        _texts: &FileTexts,
        _ops: &[Op],
        _reset: bool,
        _popups: bool,
    ) -> Result<Plan, SettingsError> {
        self.call("plan");
        self.plan.borrow().clone().expect("the test gave no plan")
    }

    fn apply(&self, _dir: &Path, _plan: &Plan, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        self.call("apply");
        self.opts.borrow_mut().push(opts.clone());
        let mut a = self.applied.borrow().clone().expect("the test gave no result");
        if opts.dry_run
            && let Ok(a) = &mut a
        {
            for f in &mut a.files {
                f.written = false;
            }
            a.undo = None;
        }
        a
    }

    fn undo(&self, _dir: &Path, _n: Option<u32>, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        self.call("undo");
        self.opts.borrow_mut().push(opts.clone());
        self.applied.borrow().clone().expect("the test gave no result")
    }

    fn restore(&self, _dir: &Path, _file: ConfigFile, opts: &WriteOpts) -> Result<Applied, SettingsError> {
        self.call("restore");
        self.opts.borrow_mut().push(opts.clone());
        self.applied.borrow().clone().expect("the test gave no result")
    }

    fn log_entries(&self, _dir: &Path, last: usize) -> Result<Vec<LogEntry>, SettingsError> {
        self.call("log_entries");
        Ok(self.log_entries.borrow().iter().take(last).cloned().collect())
    }

    fn written(&self, _dir: &Path) -> Written {
        self.written.get()
    }

    fn has_last_good(&self, _dir: &Path, _file: ConfigFile) -> bool {
        self.last_good.get()
    }

    fn append_log(&self, _log_dir: &Path, message: &str) {
        self.log.borrow_mut().push(message.to_string());
    }

    fn presets(&self, _dir: &Path) -> Result<Vec<PresetView>, SettingsError> {
        Ok(self.presets.borrow().clone())
    }

    fn widgets(&self, _dir: &Path) -> Listing {
        self.listing.borrow().clone()
    }

    fn previews(&self, presets: &[String], opts: &PreviewOpts) -> Result<Vec<Preview>, SettingsError> {
        self.call("previews");
        if let Some(e) = self.preview_error.borrow().clone() {
            return Err(e);
        }
        let mut out = self.previews.borrow().clone();
        out.retain(|p| presets.contains(&p.name) && opts.themes.contains(&p.theme));
        Ok(out)
    }

    fn bar(&self, _ctx: &Ctx) -> BarState {
        self.call("bar");
        let mut q = self.bar.borrow_mut();
        if q.len() > 1 {
            q.pop_front().unwrap_or(BarState::NotRunning)
        } else {
            q.front().cloned().unwrap_or(BarState::NotRunning)
        }
    }

    fn bar_loads(&self, _ctx: &Ctx) -> Option<u64> {
        self.loads_before.get()
    }

    fn high_contrast(&self) -> bool {
        self.high_contrast.get()
    }

    fn now_ms(&self) -> u64 {
        self.clock.get()
    }

    fn sleep_ms(&self, ms: u64) {
        self.sleeps.borrow_mut().push(ms);
        self.clock.set(self.clock.get() + ms);
    }
}
