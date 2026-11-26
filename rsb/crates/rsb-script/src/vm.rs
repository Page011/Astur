//! Creating a VM: libraries, the natives, the prelude, the sandbox, the
//! interrupt and the memory cap, in that order. Everything the prelude and
//! the natives set up happens before `sandbox(true)`, which then makes every
//! library and global table read-only.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::prelude::LuaCompiler as Compiler;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, VmState};
use rsb_core::css::PROPERTIES;
use rsb_core::services::{COMMON_FIELDS, SERVICE_SCHEMAS};

use rsb_core::popup::BUILTIN_POPUP_NAMES;

use crate::api::{API_ITEMS, HINTS, RESERVED, RESERVED_PROPS};
use crate::shared::Shared;
use crate::{Limits, natives};

pub(crate) const PRELUDE: &str = include_str!("prelude.luau");

/// Entry points the prelude returns.
pub(crate) struct Entry {
    pub load: Function,
    pub widget_spec: Function,
    pub start: Function,
    pub root: Function,
    pub flush: Function,
    pub dispose: Function,
    pub svc_event: Function,
    pub set_theme: Function,
    pub reset: Function,
    pub update_monitor: Function,
    pub popup_cmd: Function,
    pub popup_event: Function,
}

pub(crate) struct Vm {
    pub lua: Lua,
    /// The native table the prelude holds; the runtime sets `g` on it while a
    /// user input is being handled.
    pub n: Table,
    pub entry: Entry,
    pub limit: usize,
    pub soft: usize,
}

pub(crate) fn compile_prelude() -> Result<Vec<u8>, String> {
    // No debug info: the runtime's own line numbers are of no use to a widget author.
    Compiler::new().set_debug_level(0).compile(PRELUDE).map_err(|e| e.to_string())
}

fn string_list(lua: &Lua, items: impl IntoIterator<Item = String>) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for (i, s) in items.into_iter().enumerate() {
        t.raw_set(i + 1, s)?;
    }
    Ok(t)
}

fn string_set<'a>(lua: &Lua, items: impl IntoIterator<Item = &'a str>) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    for s in items {
        t.raw_set(s, true)?;
    }
    Ok(t)
}

fn build_constants(lua: &Lua, limits: &Limits, dark: bool) -> mlua::Result<Table> {
    let k = lua.create_table()?;
    k.set("max_runs", limits.max_effect_runs_per_turn)?;
    k.set("max_sort", limits.max_sort_without_comparator)?;
    k.set("dark", dark)?;
    k.set("unitless", string_set(lua, ["opacity", "font-weight", "flex-grow", "flex-shrink"])?)?;
    k.set("style_names", string_set(lua, PROPERTIES.iter().map(|p| p.name).collect::<Vec<_>>().iter().copied())?)?;
    // The CSS names with `_` are what scripts write.
    let underscored: Vec<String> = PROPERTIES.iter().map(|p| p.name.replace('-', "_")).collect();
    k.set("style_names", string_set(lua, underscored.iter().map(String::as_str))?)?;
    k.set("reserved", string_set(lua, RESERVED.iter().copied())?)?;
    k.set("reserved_props", string_set(lua, RESERVED_PROPS.iter().copied())?)?;
    k.set("builtin_popups", string_set(lua, BUILTIN_POPUP_NAMES.iter().copied())?)?;
    let hints = lua.create_table()?;
    for (name, to) in HINTS {
        hints.set(*name, *to)?;
    }
    k.set("hints", hints)?;

    let base = [
        "id",
        "class",
        "style",
        "key",
        "visible",
        "on_click",
        "on_right_click",
        "on_middle_click",
        "on_wheel",
        "on_enter",
        "on_leave",
        "on_key",
        "on_scroll",
        "on_focus",
        "on_blur",
        "tooltip",
        "menu",
        "focusable",
        "autofocus",
        "disabled",
        "checked",
        "nav",
        "reveal",
        "animate",
    ];
    let lists = lua.create_table()?;
    for (kind, extra) in [
        ("box", &[][..]),
        ("text", &["text"][..]),
        ("icon", &["name", "glyph", "svg"][..]),
        ("image", &["src", "width", "height"][..]),
        (
            "popup",
            &[
                "open",
                "name",
                "group",
                "placement",
                "align",
                "offset",
                "attach",
                "focus",
                "dismiss",
                "on_close",
                "on_open",
            ][..],
        ),
        ("menu", &["open", "items", "name", "placement", "on_close"][..]),
        ("menuitem", &["text", "name", "svg"][..]),
        ("input", &["value", "placeholder", "password", "max_length", "on_change", "on_submit"][..]),
        ("slider", &["value", "min", "max", "step", "vertical", "jump", "on_change"][..]),
        ("scroll", &[][..]),
    ] {
        lists.set(kind, string_list(lua, base.iter().chain(extra.iter()).map(|s| s.to_string()))?)?;
    }
    k.set("prop_lists", lists)?;

    let svc = lua.create_table()?;
    for s in SERVICE_SCHEMAS {
        let t = lua.create_table()?;
        t.set("fields", string_set(lua, s.fields.iter().map(|f| f.name))?)?;
        t.set("commands", string_set(lua, s.commands.iter().map(|c| c.name))?)?;
        t.set("sampled", s.sampled)?;
        let mut names: Vec<String> = s.fields.iter().map(|f| f.name.to_string()).collect();
        names.extend(COMMON_FIELDS.iter().map(|f| f.name.to_string()));
        names.extend(s.commands.iter().map(|c| c.name.to_string()));
        if s.sampled {
            names.push("interval".into());
        }
        t.set("names", string_list(lua, names)?)?;
        svc.set(s.id.name(), t)?;
    }
    k.set("svc", svc)?;

    // Names per module table, for did-you-mean.
    let names = lua.create_table()?;
    let mut top: Vec<String> = Vec::new();
    let mut groups: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
    for item in API_ITEMS {
        let (head, rest) = match item.path.split_once('.') {
            Some((h, r)) => (h, Some(r)),
            None => (item.path, None),
        };
        if !top.iter().any(|t| t == head) {
            top.push(head.to_string());
        }
        if let Some(r) = rest {
            groups.entry(head).or_default().push(r.to_string());
        }
    }
    names.set("rsb", string_list(lua, top)?)?;
    for (group, list) in groups {
        names.set(group, string_list(lua, list)?)?;
    }
    names.set("theme", string_list(lua, ["dark".to_string()])?)?;
    k.set("names", names)?;
    Ok(k)
}

fn entry_fn(e: &Table, name: &str) -> mlua::Result<Function> {
    e.get::<Function>(name)
}

/// Builds a VM whose memory is capped at `limit` bytes.
pub(crate) fn create(sh: &Rc<Shared>, prelude: &[u8], limit: usize, dark: bool, stall: bool) -> mlua::Result<Vm> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
    let load_cell: Rc<RefCell<Option<Function>>> = Rc::new(RefCell::new(None));
    let n = natives::install(&lua, sh, &load_cell)?;
    let k = build_constants(&lua, &sh.env.limits, dark)?;
    let f = lua.load(prelude).set_name("=rsb").into_function()?;
    let (e, _rsb): (Table, Table) = f.call((n.clone(), k))?;
    let entry = Entry {
        load: entry_fn(&e, "load")?,
        widget_spec: entry_fn(&e, "widget_spec")?,
        start: entry_fn(&e, "start")?,
        root: entry_fn(&e, "root")?,
        flush: entry_fn(&e, "flush")?,
        dispose: entry_fn(&e, "dispose")?,
        svc_event: entry_fn(&e, "svc_event")?,
        set_theme: entry_fn(&e, "set_theme")?,
        reset: entry_fn(&e, "reset")?,
        update_monitor: entry_fn(&e, "update_monitor")?,
        popup_cmd: entry_fn(&e, "popup_cmd")?,
        popup_event: entry_fn(&e, "popup_event")?,
    };
    *load_cell.borrow_mut() = Some(entry.load.clone());
    if stall {
        let sleep = lua.create_function(|_, ms: u64| {
            std::thread::sleep(std::time::Duration::from_millis(ms.min(10_000)));
            Ok(())
        })?;
        lua.globals().set("__test_stall", sleep)?;
    }
    lua.sandbox(true)?;
    let s = sh.clone();
    lua.set_interrupt(move |lua| {
        if s.bud.killed.get() {
            return Err(crate::shared::stopped_error());
        }
        let t = s.bud.tick.get().wrapping_add(1);
        s.bud.tick.set(t);
        if t & 15 != 0 {
            return Ok(VmState::Continue);
        }
        s.check_limits(Some(lua)).map(|_| VmState::Continue)
    });
    // A little under the cap, so a script that catches the allocator's error
    // still gets stopped: the interrupt compares against this.
    let soft = limit - limit / 32;
    lua.set_memory_limit(limit)?;
    Ok(Vm { lua, n, entry, limit, soft })
}
