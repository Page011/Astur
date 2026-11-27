//! Turning a request into a plan: every op is checked against the files as
//! they are, planned as splices one after the other (each against the text
//! the one before left), and the result is checked again by loading it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as Json, json};

use super::colors;
use super::text::Doc;
use super::toml_splice::{self as ts, Bars, Info, KeyEdit};
use super::validate::{self, BarKey, Key};
use super::{FilePlan, FileTexts, Plan, PlanContext};
use crate::color::Color;
use crate::config::{Config, ConfigSources, Position, WidgetRef, load_config_sourced};
use crate::contract::{
    AccentSet, ConfigFile, ErrorCode, FileHash, FromRef, KeptKey, Notice, NoticeCode, Op, PresetSwitch, SettingsError,
    Source, Target, Zone, fixes,
};
use crate::contrast::AccentSurfaces;
use crate::css::Stylesheet;
use crate::diag::Severity;

/// The accent's derivation: a colour and the surfaces it is read against.
pub type Derive<'a> = &'a dyn Fn(Color, &AccentSurfaces) -> AccentSet;

/// A checked change.
#[derive(Clone, Debug)]
enum Norm {
    Preset(&'static str),
    UnsetPreset,
    Position { bar: String, value: &'static str },
    Height { bar: String, value: u32 },
    Floating { bar: String, value: bool },
    Zone { bar: String, zone: Zone, list: Vec<WidgetRef> },
    UnsetBar { bar: String, key: BarKey },
    Accent(String),
    UnsetAccent,
    Move { widget: WidgetRef, from: Option<FromSel>, to: To },
    Add { widget: WidgetRef, to: To },
    Remove { widget: WidgetRef, from: Option<FromSel> },
}

#[derive(Clone, Debug)]
enum FromSel {
    Bar(String),
    At { bar: String, zone: Zone, index: Option<i32> },
}

#[derive(Clone, Debug)]
struct To {
    bar: String,
    zone: Zone,
    index: Option<i32>,
}

/// What the effective config said about itself, for one moment of the work.
struct Eff {
    cfg: Config,
    src: ConfigSources,
    messages: Vec<String>,
}

/// The editable fields of a bar, as comparable text.
const FIELDS: [&str; 13] = [
    "name",
    "monitors",
    "position",
    "height",
    "floating",
    "margin",
    "radius",
    "reserve",
    "fullscreen",
    "left",
    "center",
    "right",
    "center_on",
];

fn zone_names(list: &[crate::config::ZoneItem]) -> Vec<String> {
    list.iter().map(|z| z.widget.to_string()).collect()
}

fn field_text(bar: &crate::config::BarSpec, field: &str) -> String {
    match field {
        "name" => bar.name.clone(),
        "monitors" => format!("{:?}", bar.monitors),
        "position" => format!("{:?}", bar.position),
        "height" => bar.height.to_string(),
        "floating" => bar.floating.to_string(),
        "margin" => bar.margin.to_string(),
        "radius" => bar.radius.to_string(),
        "reserve" => bar.reserve.to_string(),
        "fullscreen" => format!("{:?}", bar.fullscreen),
        "left" => format!("{:?}", zone_names(&bar.left)),
        "center" => format!("{:?}", zone_names(&bar.center)),
        "right" => format!("{:?}", zone_names(&bar.right)),
        _ => format!("{:?}", bar.center_on),
    }
}

fn field_source(src: &crate::config::BarSources, field: &str) -> Source {
    match field {
        "name" => src.name.source,
        "monitors" => src.monitors.source,
        "position" => src.position.source,
        "height" => src.height.source,
        "floating" => src.floating.source,
        "margin" => src.margin.source,
        "radius" => src.radius.source,
        "reserve" => src.reserve.source,
        "fullscreen" => src.fullscreen.source,
        "left" => src.left.source,
        "center" => src.center.source,
        "right" => src.right.source,
        _ => src.center_on.source,
    }
}

fn zone_list_of(bar: &crate::config::BarSpec, zone: Zone) -> &[crate::config::ZoneItem] {
    match zone {
        Zone::Left => &bar.left,
        Zone::Center => &bar.center,
        Zone::Right => &bar.right,
    }
}

/// What a change meant for one field, to check against what the file now says.
#[derive(Clone, Debug, PartialEq)]
enum Intent {
    Is(String),
    Unset,
}

struct Run<'a> {
    ctx: &'a PlanContext<'a>,
    derive: Derive<'a>,
    bar: Doc,
    bar_orig: Option<String>,
    css: Doc,
    css_orig: Option<String>,
    notices: Vec<Notice>,
    bar_what: Vec<String>,
    css_what: Vec<String>,
    now_yours: Vec<(String, Zone)>,
    intents: BTreeMap<(usize, &'static str), Intent>,
    accent: Option<AccentSet>,
    accent_set: bool,
    preset_changed: bool,
    /// The preset the file must load as, when a change names one.
    preset_expect: Option<String>,
    zone_or_float_touched: bool,
    before: Option<Eff>,
}

fn internal(why: &str) -> SettingsError {
    log::warn!(target: "config", "the change did not check out: {why}");
    SettingsError::new(ErrorCode::Internal, "the change did not check out; nothing was written")
        .with_next("report it with rsb.log")
}

impl<'a> Run<'a> {
    fn eff(&self) -> Result<Eff, SettingsError> {
        load_eff(&self.bar.text, self.ctx)
    }

    fn zone_ref(&self, text: &str, eff: &Eff) -> Result<WidgetRef, SettingsError> {
        let r = validate::widget_ref(text)?;
        let placed = eff.cfg.bars.iter().any(|b| b.left.iter().chain(&b.center).chain(&b.right).any(|z| z.widget == r));
        if placed || (self.ctx.widget_exists)(&r.name) {
            return Ok(r);
        }
        let names: Vec<String> = eff
            .cfg
            .bars
            .iter()
            .flat_map(|b| b.left.iter().chain(&b.center).chain(&b.right).map(|z| z.widget.name.clone()))
            .collect();
        let hint = crate::suggest::hint(&r.name, names.iter().map(String::as_str));
        Err(SettingsError::new(ErrorCode::UnknownWidget, format!("no widget '{}'{hint}", r.name))
            .with_next("rsb settings widgets lists them"))
    }

    // ------------------------------------------------------------ bar lookup

    fn resolve_bar(&self, eff: &Eff, token: &str) -> Result<usize, SettingsError> {
        validate::bar_token(token)?;
        if let Some(i) = eff.cfg.bars.iter().position(|b| b.name == token) {
            return Ok(i);
        }
        if token.bytes().all(|b| b.is_ascii_digit())
            && let Ok(i) = token.parse::<usize>()
            && i < eff.cfg.bars.len()
        {
            return Ok(i);
        }
        let names: Vec<&str> = eff.cfg.bars.iter().map(|b| b.name.as_str()).collect();
        let listed = if names.is_empty() { "no bars".to_string() } else { format!("bars: {}", names.join(", ")) };
        Err(SettingsError::new(ErrorCode::UnknownBar, format!("no bar called '{token}' ({listed})"))
            .with_next("pick one of the bars listed"))
    }

    // ------------------------------------------------------------ bar.toml tables

    /// Makes sure bar `idx` has a live `[[bar]]` table; returns the parse.
    fn ensure_table(&mut self, eff: &Eff, idx: usize) -> Result<Info, SettingsError> {
        let info = ts::parse(&self.bar.text)?;
        match &info.bars {
            Bars::Unsupported { at, why } => Err(ts::unsupported(&self.bar.text, *at, why)),
            Bars::Empty => Err(internal("an empty bar list has no bar to change")),
            Bars::Tables(t) if t.len() > idx => Ok(info),
            Bars::Tables(_) => Err(internal("the bar is not in the file")),
            Bars::Absent => {
                let count = eff.cfg.bars.len().max(idx + 1);
                let (splices, in_place) = ts::add_bar_tables(&self.bar.text, &info, count);
                if in_place {
                    let edit = KeyEdit {
                        splices: Vec::new(),
                        example_line: Some(super::text::line_number(&self.bar.text, splices[0].start)),
                    };
                    self.live_notice(&edit);
                }
                self.bar.apply(&splices);
                self.notices.push(
                    Notice::new(
                        NoticeCode::BarsNowListed,
                        format!(
                            "bar.toml now lists {count} bar{}; a preset with more bars would show only {}",
                            if count == 1 { "" } else { "s" },
                            if count == 1 { "this one" } else { "these" }
                        ),
                    )
                    .with_file(ConfigFile::BarToml)
                    .with_extra("count", json!(count)),
                );
                let info = ts::parse(&self.bar.text)?;
                match &info.bars {
                    Bars::Tables(t) if t.len() > idx => Ok(info),
                    _ => Err(internal("the bar tables were not added")),
                }
            }
        }
    }

    fn live_notice(&mut self, edit: &KeyEdit) {
        if let Some(line) = edit.example_line {
            self.notices.push(
                Notice::new(
                    NoticeCode::ExampleTurnedLive,
                    format!("line {line} of bar.toml was a commented example; it is now a live line"),
                )
                .with_file(ConfigFile::BarToml)
                .with_extra("line", json!(line)),
            );
        }
    }

    fn set_bar_value(&mut self, eff: &Eff, idx: usize, key: &str, value: &str) -> Result<(), SettingsError> {
        let info = self.ensure_table(eff, idx)?;
        let table = &info.tables()[idx];
        let edit = ts::set_bar_key(&self.bar.text, &info, table, key, value)?;
        self.bar.apply(&edit.splices);
        self.live_notice(&edit);
        Ok(())
    }

    fn unset_bar_value(&mut self, idx: usize, key: &str) -> Result<bool, SettingsError> {
        let info = ts::parse(&self.bar.text)?;
        match &info.bars {
            Bars::Unsupported { at, why } => return Err(ts::unsupported(&self.bar.text, *at, why)),
            Bars::Tables(t) if idx < t.len() => {
                if t[idx].has_subtable {
                    return Err(ts::unsupported(
                        &self.bar.text,
                        t[idx].header,
                        "dotted keys under [[bar]] cannot be changed here",
                    ));
                }
                if let Some(e) = t[idx].entry(key) {
                    let s = ts::unset_entry(&self.bar.text, e);
                    self.bar.apply(&[s]);
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    // ------------------------------------------------------------ zones

    /// Rewrites zone `zone` of bar `idx` so its effective list is `new`.
    fn write_zone(&mut self, eff: &Eff, idx: usize, zone: Zone, new: &[String]) -> Result<(), SettingsError> {
        let info = self.ensure_table(eff, idx)?;
        let table = &info.tables()[idx];
        if table.has_subtable {
            return Err(ts::unsupported(
                &self.bar.text,
                table.header,
                "dotted keys under [[bar]] cannot be changed here",
            ));
        }
        let name = zone.name();
        let Some(entry) = table.entry(name) else {
            let value = validate::zone_text(new.iter());
            let edit = ts::set_bar_key(&self.bar.text, &info, table, name, &value)?;
            self.bar.apply(&edit.splices);
            self.live_notice(&edit);
            let bar_name = eff.cfg.bars[idx].name.clone();
            if !self.now_yours.contains(&(bar_name.clone(), zone)) {
                self.now_yours.push((bar_name, zone));
            }
            return Ok(());
        };
        let items: Option<Vec<String>> = (entry.kind == ts::Kind::Array)
            .then(|| entry.items.iter().map(|i| i.text.clone().filter(|t| WidgetRef::parse(t).is_ok())).collect())
            .flatten();
        let Some(old) = items else {
            return Err(ts::unsupported(
                &self.bar.text,
                entry.key_start,
                &format!("'{name}' is not a list of widget names; change it by hand"),
            ));
        };
        for step in diff_steps(&old, new) {
            let info = ts::parse(&self.bar.text)?;
            let table = &info.tables()[idx];
            let Some(entry) = table.entry(name) else { return Err(internal("the zone list went missing")) };
            match step {
                Step::Remove(i) => {
                    let s = ts::array_remove(&self.bar.text, entry, i);
                    self.bar.apply(&[s]);
                }
                Step::Insert(i, item) => {
                    let splices = ts::array_insert(&self.bar.text, entry, i, &validate::quoted(&item));
                    self.bar.apply(&splices);
                }
            }
        }
        Ok(())
    }

    /// A user `center_on` that names a widget no longer in `center` is removed.
    fn unpin(&mut self, eff_bar_name: &str, idx: usize) -> Result<(), SettingsError> {
        let eff = self.eff()?;
        let Some(bar) = eff.cfg.bars.get(idx) else { return Ok(()) };
        let info = ts::parse(&self.bar.text)?;
        let Bars::Tables(tables) = &info.bars else { return Ok(()) };
        let Some(table) = tables.get(idx) else { return Ok(()) };
        let Some(entry) = table.entry("center_on") else { return Ok(()) };
        let Some(text) = self.bar.text.get(entry.val_start..entry.val_end) else { return Ok(()) };
        let pin = text.trim().trim_matches(['"', '\'']);
        let Ok(pinned) = WidgetRef::parse(pin) else { return Ok(()) };
        if bar.center.iter().any(|z| z.widget == pinned) {
            return Ok(());
        }
        let s = ts::unset_entry(&self.bar.text, entry);
        self.bar.apply(&[s]);
        self.notices.push(
            Notice::new(
                NoticeCode::Unpinned,
                format!("center_on named '{pinned}', which left the centre zone; that line was removed"),
            )
            .with_file(ConfigFile::BarToml)
            .with_extra("widget", json!(pinned.to_string()))
            .with_extra("bar", json!(eff_bar_name)),
        );
        Ok(())
    }

    /// Applies `new` to zone `zone` of bar `idx`, then keeps the pin valid.
    fn change_zone(&mut self, idx: usize, zone: Zone, new: Vec<String>) -> Result<(), SettingsError> {
        let eff = self.eff()?;
        let bar = &eff.cfg.bars[idx];
        let old = zone_names(zone_list_of(bar, zone));
        let bar_name = bar.name.clone();
        if old == new {
            self.intents.insert((idx, zone_field(zone)), Intent::Is(format!("{new:?}")));
            return Ok(());
        }
        self.write_zone(&eff, idx, zone, &new)?;
        self.intents.insert((idx, zone_field(zone)), Intent::Is(format!("{new:?}")));
        self.zone_or_float_touched = true;
        self.bar_what.push(format!(
            "bar {bar_name}: {} {} -> {}",
            zone.name(),
            validate::zone_text(old.iter()),
            validate::zone_text(new.iter())
        ));
        if zone == Zone::Center {
            self.unpin(&bar_name, idx)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ ops

    fn find_placed(
        &self,
        eff: &Eff,
        widget: &WidgetRef,
        from: &Option<FromSel>,
        removing: bool,
    ) -> Result<(usize, Zone, usize), SettingsError> {
        let text = widget.to_string();
        let from_bar = match from {
            Some(FromSel::Bar(b)) => Some(self.resolve_bar(eff, b)?),
            Some(FromSel::At { bar, .. }) => Some(self.resolve_bar(eff, bar)?),
            None => None,
        };
        let mut found: Vec<(usize, Zone, usize)> = Vec::new();
        for (i, bar) in eff.cfg.bars.iter().enumerate() {
            if from_bar.is_some_and(|f| f != i) {
                continue;
            }
            for zone in Zone::ALL {
                for (k, item) in zone_list_of(bar, zone).iter().enumerate() {
                    if item.widget.to_string() == text {
                        found.push((i, zone, k));
                    }
                }
            }
        }
        let everywhere = found.clone();
        if let Some(FromSel::At { zone, index, .. }) = from {
            found.retain(|(_, z, k)| z == zone && index.is_none_or(|i| i < 0 || *k == i as usize));
        }
        let place = from_bar.map_or("any bar".to_string(), |i| format!("bar '{}'", eff.cfg.bars[i].name));
        let Some(first) = found.first().copied() else {
            let next = if removing {
                "it is not on the bar; rsb settings get lists what each zone holds"
            } else {
                "add it instead of moving it"
            };
            if let (Some(FromSel::At { .. }), false) = (from, everywhere.is_empty()) {
                let at: Vec<String> =
                    everywhere.iter().map(|(i, z, k)| format!("{}.{}:{k}", eff.cfg.bars[*i].name, z.name())).collect();
                return Err(SettingsError::new(
                    ErrorCode::WidgetNotPlaced,
                    format!("'{text}' is not at that place; it is at {}", at.join(", ")),
                )
                .with_next("name one of those places"));
            }
            return Err(SettingsError::new(ErrorCode::WidgetNotPlaced, format!("'{text}' is in no zone of {place}"))
                .with_next(next));
        };
        let bars: BTreeSet<usize> = found.iter().map(|f| f.0).collect();
        if bars.len() > 1 {
            let names: Vec<&str> = bars.iter().map(|i| eff.cfg.bars[*i].name.as_str()).collect();
            return Err(SettingsError::new(
                ErrorCode::AmbiguousWidget,
                format!("'{text}' is in bars {}", names.join(" and ")),
            )
            .with_next("say which bar with --from"));
        }
        let zones: BTreeSet<&str> = found.iter().map(|f| f.1.name()).collect();
        if zones.len() > 1 {
            let names: Vec<&str> = zones.into_iter().collect();
            return Err(SettingsError::new(
                ErrorCode::AmbiguousWidget,
                format!("'{text}' is in the {} zones of bar '{}'", names.join(" and "), eff.cfg.bars[first.0].name),
            )
            .with_next("say which zone with --from <bar>.<zone>"));
        }
        if found.len() > 1 {
            return Err(SettingsError::new(
                ErrorCode::AmbiguousWidget,
                format!("'{text}' is placed {} times in the {} zone", found.len(), first.1.name()),
            )
            .with_next("say which one with --from <bar>.<zone>:<index>"));
        }
        Ok(first)
    }

    fn target_index(&self, to: &To, len: usize) -> Result<usize, SettingsError> {
        match to.index {
            None | Some(-1) => Ok(len),
            Some(i) if i >= 0 && (i as usize) <= len => Ok(i as usize),
            Some(i) => Err(SettingsError::new(
                ErrorCode::BadValue,
                format!("index {i} is not inside the {len} widgets of that zone (0 to {len}, or -1 for the end)"),
            )),
        }
    }

    fn apply(&mut self, op: &Norm) -> Result<(), SettingsError> {
        match op {
            Norm::Preset(name) => {
                let eff = self.eff()?;
                let info = ts::parse(&self.bar.text)?;
                let edit = ts::set_root(&self.bar.text, &info, "preset", &validate::quoted(name))?;
                self.bar.apply(&edit.splices);
                self.live_notice(&edit);
                self.bar_what.push(format!("preset {} -> {name}", eff.cfg.preset));
                self.preset_changed = true;
                self.preset_expect = Some((*name).to_string());
            }
            Norm::UnsetPreset => {
                let eff = self.eff()?;
                let info = ts::parse(&self.bar.text)?;
                if let Some(e) = info.root_entry("preset") {
                    let s = ts::unset_entry(&self.bar.text, e);
                    self.bar.apply(&[s]);
                    let after = self.eff()?;
                    self.bar_what.push(format!("preset {} -> {}", eff.cfg.preset, after.cfg.preset));
                    self.preset_changed = true;
                    self.preset_expect = Some(crate::config::DEFAULT_PRESET.to_string());
                }
            }
            Norm::Position { bar, value } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, bar)?;
                self.set_bar_value(&eff, idx, "position", &validate::quoted(value))?;
                let old = format!("{:?}", eff.cfg.bars[idx].position).to_ascii_lowercase();
                self.bar_what.push(format!("bar {}: position {old} -> {value}", eff.cfg.bars[idx].name));
                self.intents.insert(
                    (idx, "position"),
                    Intent::Is(format!("{:?}", if *value == "top" { Position::Top } else { Position::Bottom })),
                );
            }
            Norm::Height { bar, value } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, bar)?;
                self.set_bar_value(&eff, idx, "height", &value.to_string())?;
                self.bar_what
                    .push(format!("bar {}: height {} -> {value}", eff.cfg.bars[idx].name, eff.cfg.bars[idx].height));
                self.intents.insert((idx, "height"), Intent::Is(value.to_string()));
            }
            Norm::Floating { bar, value } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, bar)?;
                self.set_bar_value(&eff, idx, "floating", if *value { "true" } else { "false" })?;
                self.bar_what.push(format!(
                    "bar {}: floating {} -> {value}",
                    eff.cfg.bars[idx].name, eff.cfg.bars[idx].floating
                ));
                self.intents.insert((idx, "floating"), Intent::Is(value.to_string()));
                self.zone_or_float_touched = true;
            }
            Norm::Zone { bar, zone, list } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, bar)?;
                for r in list {
                    self.zone_ref(&r.to_string(), &eff)?;
                }
                let new: Vec<String> = list.iter().map(ToString::to_string).collect();
                self.change_zone(idx, *zone, new)?;
            }
            Norm::UnsetBar { bar, key } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, bar)?;
                if self.unset_bar_value(idx, key.name())? {
                    let after = self.eff()?;
                    let (b, a) = (&eff.cfg.bars[idx], &after.cfg.bars[idx]);
                    self.bar_what.push(format!(
                        "bar {}: {} {} -> {}",
                        b.name,
                        key.name(),
                        field_text(b, key.name()).replace('"', ""),
                        field_text(a, key.name()).replace('"', "")
                    ));
                    self.intents.insert((idx, bar_key_field(*key)), Intent::Unset);
                    if matches!(key, BarKey::Zone(_) | BarKey::Floating) {
                        self.zone_or_float_touched = true;
                    }
                    if *key == BarKey::Zone(Zone::Center) {
                        self.unpin(&a.name.clone(), idx)?;
                    }
                }
            }
            Norm::Add { widget, to } => {
                let eff = self.eff()?;
                let idx = self.resolve_bar(&eff, &to.bar)?;
                let widget = self.zone_ref(&widget.to_string(), &eff)?;
                let already: Vec<String> = Zone::ALL
                    .into_iter()
                    .flat_map(|z| {
                        zone_list_of(&eff.cfg.bars[idx], z)
                            .iter()
                            .enumerate()
                            .filter(|(_, item)| item.widget.to_string() == widget.to_string())
                            .map(move |(k, _)| format!("{}:{k}", z.name()))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let mut list = zone_names(zone_list_of(&eff.cfg.bars[idx], to.zone));
                let at = self.target_index(to, list.len())?;
                list.insert(at, widget.to_string());
                self.change_zone(idx, to.zone, list)?;
                if !already.is_empty() {
                    self.notices.push(
                        Notice::new(
                            NoticeCode::AlreadyPlaced,
                            format!(
                                "'{widget}' was already on bar '{}' ({}); it is there twice now",
                                eff.cfg.bars[idx].name,
                                already.join(", ")
                            ),
                        )
                        .with_file(ConfigFile::BarToml)
                        .with_next("rsb settings remove the extra copy if that was not meant")
                        .with_extra("places", json!(already)),
                    );
                }
            }
            Norm::Remove { widget, from } => {
                let eff = self.eff()?;
                let (idx, zone, k) = self.find_placed(&eff, widget, from, true)?;
                let mut list = zone_names(zone_list_of(&eff.cfg.bars[idx], zone));
                list.remove(k);
                self.change_zone(idx, zone, list)?;
            }
            Norm::Move { widget, from, to } => {
                let eff = self.eff()?;
                let (sidx, szone, k) = self.find_placed(&eff, widget, from, false)?;
                let didx = self.resolve_bar(&eff, &to.bar)?;
                if (sidx, szone) == (didx, to.zone) {
                    let mut list = zone_names(zone_list_of(&eff.cfg.bars[sidx], szone));
                    let item = list.remove(k);
                    let at = self.target_index(to, list.len())?;
                    list.insert(at, item);
                    self.change_zone(sidx, szone, list)?;
                } else {
                    let mut dest = zone_names(zone_list_of(&eff.cfg.bars[didx], to.zone));
                    let at = self.target_index(to, dest.len())?;
                    dest.insert(at, widget.to_string());
                    let mut src = zone_names(zone_list_of(&eff.cfg.bars[sidx], szone));
                    src.remove(k);
                    // Take it out of its old place first so a failure leaves nothing half done.
                    self.change_zone(sidx, szone, src)?;
                    self.change_zone(didx, to.zone, dest)?;
                }
            }
            Norm::Accent(hex) => {
                let color = Color::parse(hex).map_err(|_| internal("a checked colour does not parse"))?;
                let set = (self.derive)(color, self.ctx.surfaces);
                let before = colors::accent_of(&self.css.text);
                let existing = colors::find_block(&self.css.text)?;
                // The block is rewritten whole: a colour a person changed inside it goes with it.
                if let Some(b) = &existing
                    && self.block_edited(b)
                {
                    self.notices.push(
                        Notice::new(
                            NoticeCode::BlockEditReplaced,
                            "a colour edited by hand inside the colours block was replaced with the shades of the accent",
                        )
                        .with_file(ConfigFile::StyleCss)
                        .with_next("rsb settings undo brings it back; put your own colours below the end line to keep them"),
                    );
                }
                let eol = super::text::eol_of(&self.css.text);
                let s = colors::put_block(&self.css.text, existing.as_ref(), &colors::render(&set, eol));
                self.css.apply(&[s]);
                self.css_what.push(format!("accent {} -> {}", before.as_deref().unwrap_or("none"), set.value));
                self.accent = Some(set);
                self.accent_set = true;
            }
            Norm::UnsetAccent => {
                if let Some(b) = colors::find_block(&self.css.text)? {
                    let before = b.dark[0].clone();
                    self.note_block_removed(&b);
                    let s = colors::remove_block(&self.css.text, &b);
                    self.css.apply(&[s]);
                    self.css_what.push(format!("accent {before} -> none"));
                }
                self.accent = None;
                self.accent_set = false;
            }
        }
        Ok(())
    }

    /// Whether a colour of the block differs from the shades its own accent gives, which is what a hand edit inside
    /// the block looks like. A block whose accent does not parse is not judged.
    fn block_edited(&self, b: &colors::Block) -> bool {
        let Ok(own) = Color::parse(&b.dark[0]) else { return false };
        let (dark, light) = colors::values_of(&(self.derive)(own, self.ctx.surfaces));
        b.dark != dark || b.light != light
    }

    /// Says so when removing the block takes a hand-edited colour with it.
    fn note_block_removed(&mut self, b: &colors::Block) {
        if self.block_edited(b) {
            self.notices.push(
                Notice::new(
                    NoticeCode::BlockEditReplaced,
                    "a colour edited by hand inside the colours block was removed with the block",
                )
                .with_file(ConfigFile::StyleCss)
                .with_next("rsb settings undo brings it back"),
            );
        }
    }

    /// `reset`: the user's bar keys, zone lists and accent block go.
    fn reset(&mut self) -> Result<(), SettingsError> {
        let info = ts::parse(&self.bar.text)?;
        let count = info.tables().len();
        for idx in 0..count {
            for key in BarKey::ALL {
                self.unset_bar_value(idx, key.name())?;
            }
        }
        if let Some(b) = colors::find_block(&self.css.text)? {
            let before = b.dark[0].clone();
            self.note_block_removed(&b);
            let s = colors::remove_block(&self.css.text, &b);
            self.css.apply(&[s]);
            self.css_what.push(format!("accent {before} -> none"));
        }
        Ok(())
    }
}

fn zone_field(zone: Zone) -> &'static str {
    match zone {
        Zone::Left => "left",
        Zone::Center => "center",
        Zone::Right => "right",
    }
}

fn bar_key_field(key: BarKey) -> &'static str {
    match key {
        BarKey::Position => "position",
        BarKey::Height => "height",
        BarKey::Floating => "floating",
        BarKey::Zone(z) => zone_field(z),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Remove(usize),
    Insert(usize, String),
}

/// The removals and insertions that turn `old` into `new`, from a longest
/// common subsequence, with indices as the list stands when each step runs.
fn diff_steps(old: &[String], new: &[String]) -> Vec<Step> {
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut at) = (0usize, 0usize, 0usize);
    let mut steps = Vec::new();
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            i += 1;
            j += 1;
            at += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            steps.push(Step::Remove(at));
            i += 1;
        } else {
            steps.push(Step::Insert(at, new[j].clone()));
            j += 1;
            at += 1;
        }
    }
    steps
}

fn load_eff(text: &str, ctx: &PlanContext<'_>) -> Result<Eff, SettingsError> {
    ts::parse(text)?;
    let loaded = load_config_sourced(Some((text, "bar.toml")), ctx.preset_toml);
    let messages = loaded.load.diagnostics.iter().map(|d| d.message.clone()).collect();
    match (loaded.load.config, loaded.sources) {
        (Some(cfg), Some(src)) => Ok(Eff { cfg, src, messages }),
        _ => {
            let why = loaded
                .load
                .diagnostics
                .iter()
                .find(|d| d.severity == Severity::Error)
                .map(|d| match &d.pos {
                    Some(p) => format!("{p}: {}", d.message),
                    None => d.message.clone(),
                })
                .unwrap_or_else(|| "bar.toml cannot be read".to_string());
            Err(SettingsError::new(ErrorCode::ConfigTooNew, why).with_file(ConfigFile::BarToml).with_next("update RSB"))
        }
    }
}

// ---------------------------------------------------------------- normalising

fn norm_target(t: &Target) -> Result<To, SettingsError> {
    validate::bar_token(&t.bar)?;
    Ok(To { bar: t.bar.clone(), zone: t.zone, index: t.index })
}

fn norm_from(f: &Option<FromRef>) -> Result<Option<FromSel>, SettingsError> {
    Ok(match f {
        None => None,
        Some(FromRef::Bar(b)) => {
            validate::bar_token(b)?;
            Some(FromSel::Bar(b.clone()))
        }
        Some(FromRef::At(t)) => {
            validate::bar_token(&t.bar)?;
            Some(FromSel::At { bar: t.bar.clone(), zone: t.zone, index: t.index })
        }
    })
}

fn norm_op(op: &Op, bar_names: &[String]) -> Result<Norm, SettingsError> {
    match op {
        Op::Set { key, value } => match validate::parse_key(key, bar_names)? {
            Key::Preset => Ok(Norm::Preset(validate::preset(key, value)?)),
            Key::Accent => Ok(Norm::Accent(validate::accent(key, value)?)),
            Key::Bar { bar, key: BarKey::Position } => {
                Ok(Norm::Position { bar, value: validate::position(key, value)? })
            }
            Key::Bar { bar, key: BarKey::Height } => Ok(Norm::Height { bar, value: validate::height(key, value)? }),
            Key::Bar { bar, key: BarKey::Floating } => {
                Ok(Norm::Floating { bar, value: validate::floating(key, value)? })
            }
            Key::Bar { bar, key: BarKey::Zone(zone) } => {
                Ok(Norm::Zone { bar, zone, list: validate::zone_list(key, value)? })
            }
        },
        Op::Unset { key } => match validate::parse_key(key, bar_names)? {
            Key::Preset => Ok(Norm::UnsetPreset),
            Key::Accent => Ok(Norm::UnsetAccent),
            Key::Bar { bar, key } => Ok(Norm::UnsetBar { bar, key }),
        },
        Op::Move { widget, from, to } => {
            Ok(Norm::Move { widget: validate::widget_ref(widget)?, from: norm_from(from)?, to: norm_target(to)? })
        }
        Op::Add { widget, to } => Ok(Norm::Add { widget: validate::widget_ref(widget)?, to: norm_target(to)? }),
        Op::Remove { widget, from } => {
            Ok(Norm::Remove { widget: validate::widget_ref(widget)?, from: norm_from(from)? })
        }
        Op::Undo { .. } | Op::Restore { .. } => Err(SettingsError::new(
            ErrorCode::BadUsage,
            "undo and restore are not changes; use rsb settings undo or rsb settings restore",
        )),
    }
}

/// The request shape of a checked change: tokens rendered from the checked
/// values.
fn norm_to_op(n: &Norm) -> Op {
    let target = |t: &To| Target { bar: t.bar.clone(), zone: t.zone, index: t.index };
    let from = |f: &Option<FromSel>| {
        f.as_ref().map(|f| match f {
            FromSel::Bar(b) => FromRef::Bar(b.clone()),
            FromSel::At { bar, zone, index } => FromRef::At(Target { bar: bar.clone(), zone: *zone, index: *index }),
        })
    };
    match n {
        Norm::Preset(p) => Op::Set { key: "preset".into(), value: json!(p) },
        Norm::UnsetPreset => Op::Unset { key: "preset".into() },
        Norm::Position { bar, value } => Op::Set { key: format!("bar.{bar}.position"), value: json!(value) },
        Norm::Height { bar, value } => Op::Set { key: format!("bar.{bar}.height"), value: json!(value) },
        Norm::Floating { bar, value } => Op::Set { key: format!("bar.{bar}.floating"), value: json!(value) },
        Norm::Zone { bar, zone, list } => Op::Set {
            key: format!("bar.{bar}.{}", zone.name()),
            value: json!(list.iter().map(ToString::to_string).collect::<Vec<_>>()),
        },
        Norm::UnsetBar { bar, key } => Op::Unset { key: format!("bar.{bar}.{}", key.name()) },
        Norm::Accent(hex) => Op::Set { key: "color.accent".into(), value: json!(hex) },
        Norm::UnsetAccent => Op::Unset { key: "color.accent".into() },
        Norm::Move { widget, from: f, to } => Op::Move { widget: widget.to_string(), from: from(f), to: target(to) },
        Norm::Add { widget, to } => Op::Add { widget: widget.to_string(), to: target(to) },
        Norm::Remove { widget, from: f } => Op::Remove { widget: widget.to_string(), from: from(f) },
    }
}

fn touches_css(n: &Norm) -> bool {
    matches!(n, Norm::Accent(_) | Norm::UnsetAccent)
}

// ---------------------------------------------------------------- the plan

pub(crate) fn plan(
    texts: &FileTexts,
    ops: &[Op],
    ctx: &PlanContext<'_>,
    derive: Derive<'_>,
) -> Result<Plan, SettingsError> {
    if ops.is_empty() {
        return Err(SettingsError::new(ErrorCode::BadUsage, "the change request holds no changes")
            .with_next("send at least one change"));
    }
    // Names for hints and for finding a bar: the effective bars of the file as it is.
    let strip = |t: &Option<String>| t.as_deref().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s).to_string());
    let (bar_orig, css_orig) = (strip(&texts.bar_toml), strip(&texts.style_css));
    let template = |f: ConfigFile| (ctx.template)(f);
    let bar_start = bar_orig.clone().unwrap_or_else(|| template(ConfigFile::BarToml));
    let css_start = css_orig.clone().unwrap_or_else(|| template(ConfigFile::StyleCss));
    let names: Vec<String> =
        load_eff(&bar_start, ctx).map(|e| e.cfg.bars.iter().map(|b| b.name.clone()).collect()).unwrap_or_default();
    let norms: Vec<Norm> = ops.iter().map(|o| norm_op(o, &names)).collect::<Result<_, _>>()?;
    if ctx.reset && !norms.iter().any(|n| matches!(n, Norm::Preset(_))) {
        return Err(SettingsError::new(ErrorCode::BadUsage, "reset goes with setting a preset")
            .with_next("use set preset <name> with reset"));
    }
    let needs_bar = norms.iter().any(|n| !touches_css(n)) || ctx.reset;
    let needs_css = norms.iter().any(touches_css) || ctx.reset;

    // A broken file refuses what it feeds; the other file stays writable.
    let before = if needs_bar { Some(load_eff(&bar_start, ctx)?) } else { None };
    if needs_css && css_orig.is_some() {
        let (_, diags) = Stylesheet::parse(&css_start, "style.css");
        if let Some(d) = diags.iter().find(|d| d.severity == Severity::Error) {
            let at = d.pos.as_ref().map_or("style.css".to_string(), |p| p.to_string());
            return Err(SettingsError::new(ErrorCode::FileBroken, format!("{at}: {}", d.message))
                .with_file(ConfigFile::StyleCss)
                .with_at(at)
                .with_next("fix that line, or run: rsb settings restore style.css (your broken copy is kept)"));
        }
    }

    let mut run = Run {
        ctx,
        derive,
        bar: Doc::new(&bar_start),
        bar_orig: bar_orig.clone(),
        css: Doc::new(&css_start),
        css_orig: css_orig.clone(),
        notices: Vec::new(),
        bar_what: Vec::new(),
        css_what: Vec::new(),
        now_yours: Vec::new(),
        intents: BTreeMap::new(),
        accent: None,
        accent_set: false,
        preset_changed: false,
        preset_expect: None,
        zone_or_float_touched: false,
        before,
    };
    let preset_wanted = norms.iter().rev().find_map(|n| if let Norm::Preset(p) = n { Some(*p) } else { None });
    for n in &norms {
        run.apply(n)?;
        if ctx.reset && matches!(n, Norm::Preset(_)) {
            run.reset()?;
        }
    }
    finish(run, preset_wanted, norms)
}

fn finish(mut run: Run<'_>, preset_wanted: Option<&'static str>, norms: Vec<Norm>) -> Result<Plan, SettingsError> {
    let ctx = run.ctx;
    let before = run.before.take();
    let mut files: Vec<FilePlan> = Vec::new();
    let mut preset_switch = None;

    // bar.toml
    if run.bar.text != run.bar.original() {
        let after = load_eff(&run.bar.text, ctx).map_err(|e| {
            log::warn!(target: "config", "the new bar.toml does not load: {}", e.code);
            internal("the new bar.toml does not load")
        })?;
        if let Some(before) = before.as_ref() {
            verify_bar(before, &after, &run)?;
            if let Some(to) = preset_wanted {
                preset_switch = Some(switch_report(before, &after, to, &run, ctx));
            }
        }
        if !run.now_yours.is_empty() {
            let zones: Vec<Json> = run.now_yours.iter().map(|(b, z)| json!({"bar": b, "zone": z.name()})).collect();
            let keys: Vec<String> = run.now_yours.iter().map(|(b, z)| format!("bar.{b}.{}", z.name())).collect();
            let commands: Vec<String> = keys.iter().map(|k| format!("rsb settings unset {k}")).collect();
            run.notices.push(
                Notice::new(NoticeCode::NowYours, "a zone list the preset set is now written in bar.toml")
                    .with_file(ConfigFile::BarToml)
                    .with_next(format!("to follow the preset's list again, run: {}", commands.join("; ")))
                    .with_extra("fix", Json::Array(keys.iter().map(|k| fixes::unset(k)).collect()))
                    .with_extra("zones", Json::Array(zones)),
            );
        }
        popup_notices(&mut run, before.as_ref(), &after);
        let created = run.bar_orig.is_none();
        let base = run.bar_orig.as_ref().map_or(FileHash::Missing, |t| FileHash::of(t.as_bytes()));
        if created {
            run.notices.push(file_created(ConfigFile::BarToml));
        }
        files.push(FilePlan {
            file: ConfigFile::BarToml,
            base,
            created,
            bom: false,
            splices: run.bar.splices(),
            after: run.bar.text.clone(),
            what: run.bar_what.join("; "),
        });
    }

    // style.css
    if run.css.text != run.css.original() {
        verify_css(&run)?;
        if run.accent_set {
            accent_notices(&mut run);
        }
        let created = run.css_orig.is_none();
        let base = run.css_orig.as_ref().map_or(FileHash::Missing, |t| FileHash::of(t.as_bytes()));
        if created {
            run.notices.push(file_created(ConfigFile::StyleCss));
        }
        files.push(FilePlan {
            file: ConfigFile::StyleCss,
            base,
            created,
            bom: false,
            splices: run.css.splices(),
            after: run.css.text.clone(),
            what: run.css_what.join("; "),
        });
    }
    files.sort_by_key(|f| f.file);
    // The preset report also names the accent that stays.
    if let Some(ps) = preset_switch.as_mut() {
        ps.accent_kept = colors::accent_of(&run.css.text);
    }
    Ok(Plan {
        files,
        changes: norms.iter().map(norm_to_op).collect(),
        notices: run.notices,
        preset_switch,
        accent: if run.accent_set { run.accent } else { None },
    })
}

fn file_created(file: ConfigFile) -> Notice {
    Notice::new(NoticeCode::FileCreated, format!("{file} did not exist; it is written from the template"))
        .with_file(file)
}

fn accent_notices(run: &mut Run<'_>) {
    let sheet = (run.ctx.style_chain)(&run.css.text).unwrap_or_else(|| Stylesheet::parse(&run.css.text, "style.css").0);
    let Some(c) = colors::accent_conflicts(&sheet, &run.css.text) else { return };
    if let Some(first) = c.shadowed.first() {
        run.notices.push(
            Notice::new(
                NoticeCode::Shadowed,
                format!("a later rule sets --{} again, so the colour set here does not fully show", first.token),
            )
            .with_file(ConfigFile::StyleCss)
            .with_next(format!("move or delete the rule at {}", first.at))
            .with_extra("at", json!(first.at)),
        );
    }
    if !c.replaced.is_empty() {
        let at: Vec<&str> = c.replaced.iter().map(|c| c.at.as_str()).collect();
        run.notices.push(
            Notice::new(NoticeCode::ReplacesYours, "earlier accent lines now do nothing; the colours block beats them")
                .with_file(ConfigFile::StyleCss)
                .with_next(format!("change the accent in the colours block, or delete the lines at {}", at.join(", ")))
                .with_extra("at", json!(at)),
        );
    }
}

fn popup_notices(run: &mut Run<'_>, before: Option<&Eff>, after: &Eff) {
    let Some(before) = before else { return };
    let refs = |e: &Eff| -> BTreeSet<WidgetRef> {
        e.cfg
            .bars
            .iter()
            .flat_map(|b| b.left.iter().chain(&b.center).chain(&b.right).map(|z| z.widget.clone()))
            .collect()
    };
    let (was, is) = (refs(before), refs(after));
    let mut told: BTreeSet<String> = BTreeSet::new();
    for gone in was.difference(&is) {
        for popup in (run.ctx.widget_opens)(&gone.name) {
            let still = is.iter().any(|r| (run.ctx.widget_opens)(&r.name).contains(&popup));
            if !still && told.insert(popup.clone()) {
                // A preset switch drops widgets the person never removed: say what the preset lacks.
                let message = if run.preset_changed {
                    format!("{} has no widget that opens the {popup} menu", after.cfg.preset)
                } else {
                    format!("'{gone}' was the last widget that opens the {popup} menu")
                };
                run.notices.push(
                    Notice::new(NoticeCode::PopupUnreachable, message)
                        .with_file(ConfigFile::BarToml)
                        .with_next(format!("add {gone} back to a zone to reach the {popup} menu again"))
                        .with_extra("popup", json!(popup))
                        .with_extra("widget", json!(gone.to_string())),
                );
            }
        }
    }
}

// ---------------------------------------------------------------- checks

fn user_bar_count(text: &str) -> usize {
    ts::parse(text).map(|i| i.tables().len()).unwrap_or(0)
}

fn json_zone(list: &[crate::config::ZoneItem]) -> Json {
    Json::Array(list.iter().map(|z| json!(z.widget.to_string())).collect())
}

fn switch_report(before: &Eff, after: &Eff, to: &str, run: &Run<'_>, ctx: &PlanContext<'_>) -> PresetSwitch {
    let preset_only = load_config_sourced(Some((&format!("preset = \"{to}\"\n"), "bar.toml")), ctx.preset_toml);
    let preset_bars = preset_only.load.config.map(|c| c.bars).unwrap_or_default();
    let mut kept = Vec::new();
    for (i, bar) in after.cfg.bars.iter().enumerate() {
        let src = &after.src.bars[i];
        let from_preset = preset_bars.get(i).or(preset_bars.last());
        let mut keys: Vec<(&str, Source, Json, Option<Json>)> = Vec::new();
        keys.push((
            "position",
            src.position.source,
            json!(format!("{:?}", bar.position).to_ascii_lowercase()),
            from_preset.map(|p| json!(format!("{:?}", p.position).to_ascii_lowercase())),
        ));
        keys.push(("height", src.height.source, json!(bar.height), from_preset.map(|p| json!(p.height))));
        keys.push(("floating", src.floating.source, json!(bar.floating), from_preset.map(|p| json!(p.floating))));
        keys.push(("left", src.left.source, json_zone(&bar.left), from_preset.map(|p| json_zone(&p.left))));
        keys.push(("center", src.center.source, json_zone(&bar.center), from_preset.map(|p| json_zone(&p.center))));
        keys.push(("right", src.right.source, json_zone(&bar.right), from_preset.map(|p| json_zone(&p.right))));
        for (key, source, yours, preset) in keys {
            if source != Source::User {
                continue;
            }
            let preset = preset.unwrap_or(Json::Null);
            kept.push(KeptKey { bar: bar.name.clone(), key: key.to_string(), same: yours == preset, yours, preset });
        }
    }
    PresetSwitch {
        from: before.cfg.preset.clone(),
        to: to.to_string(),
        kept,
        bars_listed: user_bar_count(&run.bar.text) as u32,
        preset_bars: preset_bars.len() as u32,
        accent_kept: None,
    }
}

/// The messages of the diagnostics that are not about an unused option table.
fn message_counts(e: &Eff) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for msg in &e.messages {
        if !msg.ends_with("is not used by any bar") {
            *m.entry(msg.clone()).or_insert(0) += 1;
        }
    }
    m
}

/// The loaded new file must be the old one with exactly the intended change.
fn verify_bar(before: &Eff, after: &Eff, run: &Run<'_>) -> Result<(), SettingsError> {
    let changed_preset = run.preset_changed || run.ctx.reset;
    if !changed_preset && before.cfg.bars.len() != after.cfg.bars.len() {
        return Err(internal("the number of bars changed"));
    }
    if !changed_preset {
        let opts = |e: &Eff| -> BTreeMap<String, BTreeMap<String, crate::value::Value>> {
            e.cfg
                .widgets
                .iter()
                .map(|(k, v)| (k.clone(), v.values.iter().map(|(k, s)| (k.clone(), s.value.clone())).collect()))
                .collect()
        };
        if opts(before) != opts(after) {
            return Err(internal("widget options changed"));
        }
        if before.cfg.preset != after.cfg.preset {
            return Err(internal("the preset changed"));
        }
    }
    if let Some(p) = &run.preset_expect
        && &after.cfg.preset != p
    {
        return Err(internal("the preset is not the one asked for"));
    }
    let new_msgs = message_counts(after);
    let old_msgs = message_counts(before);
    for (msg, n) in &new_msgs {
        if old_msgs.get(msg).copied().unwrap_or(0) < *n && !changed_preset {
            return Err(internal("a new diagnostic appeared"));
        }
    }
    let editable = ["position", "height", "floating", "left", "center", "right"];
    for i in 0..before.cfg.bars.len().min(after.cfg.bars.len()) {
        for field in FIELDS {
            let (b, a) = (field_text(&before.cfg.bars[i], field), field_text(&after.cfg.bars[i], field));
            let src_after = field_source(&after.src.bars[i], field);
            let src_before = field_source(&before.src.bars[i], field);
            let intent = run.intents.get(&(i, field));
            let ok = match intent {
                Some(Intent::Is(v)) => &a == v,
                Some(Intent::Unset) => src_after != Source::User,
                None => {
                    let exempt = (field == "center_on" && (run.zone_or_float_touched || changed_preset))
                        || (field == "radius" && (run.zone_or_float_touched || changed_preset));
                    if exempt || (run.ctx.reset && editable.contains(&field)) {
                        true
                    } else if changed_preset {
                        src_before != Source::User || a == b
                    } else {
                        a == b
                    }
                }
            };
            if !ok {
                return Err(internal(&format!("bar {i} {field} is not what was meant")));
            }
        }
    }
    Ok(())
}

fn verify_css(run: &Run<'_>) -> Result<(), SettingsError> {
    let (_, diags) = Stylesheet::parse(&run.css.text, "style.css");
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(internal("the new style.css does not parse"));
    }
    let block = colors::find_block(&run.css.text).map_err(|_| internal("the colours block does not read back"))?;
    match (&run.accent, run.accent_set, block) {
        (Some(set), true, Some(b)) => {
            let (dark, light) = colors::values_of(set);
            if b.dark != dark || b.light != light {
                return Err(internal("the colours block is not what was meant"));
            }
        }
        (_, false, None) => {}
        _ => return Err(internal("the colours block is not what was meant")),
    }
    Ok(())
}
