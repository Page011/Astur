//! The engine: instance lifecycle, the scheduler, and the glue between the
//! messages the binary sends and the Luau VMs. One `Core` runs on the script
//! thread; tests drive it directly through `Runtime`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::{Lua, MultiValue, Table, Value as Lv};
use rsb_core::config::WidgetRef;
use rsb_core::diag::{DiagKind, Diagnostic, Plate, SourcePos};
use rsb_core::element::{InstanceId, WidgetTree};
use rsb_core::hit::{Button, ElementCall, ElementEvent, HandlerCall, InputKind};
use rsb_core::io::{IoEvent, IoEventKind, IoGrant};
use rsb_core::msg::{
    HubEvent, HubPort, HubRequest, InstanceSpec, ScriptIn, ScriptOut, ScriptSink, WidgetSource, WidgetState,
    WidgetStatus,
};
use rsb_core::paths::widget_files;
use rsb_core::permissions::Permissions;
use rsb_core::popup::PopupAction;
use rsb_core::services::{ServiceEvent, ServiceStatus};
use rsb_core::time::{TimeZone, until_boundary};
use rsb_core::value::Value;

use crate::ScriptOptions;
use crate::convert::{Walk, from_lua, to_lua};
use crate::natives::suggest;
use crate::shared::*;
use crate::snapshot::snapshot;
use crate::vm::{self, Vm};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum VmKey {
    Builtin,
    User(String),
}

enum Why {
    Stop(Stop),
    /// A raw Luau error text, `file:line: message`.
    Script(String),
    Diag(Diagnostic),
}

struct Resolved {
    rel: String,
    src: String,
    builtin: bool,
    /// A user file with the name of a built-in widget.
    replaces_builtin: bool,
}

pub(crate) struct Core {
    sh: Rc<Shared>,
    prelude: Vec<u8>,
    vms: HashMap<VmId, Vm>,
    vm_keys: HashMap<VmKey, VmId>,
    vm_of_inst: HashMap<InstanceId, VmKey>,
    next_vm: VmId,
    rr_last: Option<InstanceId>,
    /// A popup closed since the last turn ended: its tree is garbage now, so the VMs are swept once.
    sweep: bool,
}

fn split_pos(msg: &str) -> (Option<SourcePos>, String) {
    let mut parts = msg.splitn(3, ':');
    if let (Some(file), Some(line), Some(rest)) = (parts.next(), parts.next(), parts.next())
        && !file.is_empty()
        && !file.contains(' ')
        && let Ok(n) = line.trim().parse::<u32>()
    {
        return (Some(SourcePos::new(file, n, 0)), rest.trim().to_string());
    }
    (None, msg.trim().to_string())
}

fn parent_dir(rel: &str) -> String {
    rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default()
}

fn pop_args(lua: &Lua, args: &[Value]) -> mlua::Result<MultiValue> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        out.push(to_lua(lua, a, false)?);
    }
    Ok(MultiValue::from_vec(out))
}

impl Core {
    pub(crate) fn new(options: ScriptOptions, hub: Box<dyn HubPort>, out: Box<dyn ScriptSink>) -> Core {
        let env = Env {
            sources: options.sources,
            clock: options.clock,
            config_dir: options.config_dir,
            path_env: options.path_env,
            limits: options.limits,
            thread_time: options.thread_time,
            heartbeat: options.heartbeat,
        };
        let sh = Rc::new(Shared::new(env, hub, out));
        sh.st.borrow_mut().dark = options.dark;
        let prelude = vm::compile_prelude().unwrap_or_else(|e| {
            log::error!("the widget runtime failed to compile: {e}");
            Vec::new()
        });
        Core {
            sh,
            prelude,
            vms: HashMap::new(),
            vm_keys: HashMap::new(),
            vm_of_inst: HashMap::new(),
            next_vm: 0,
            rr_last: None,
            sweep: false,
        }
    }

    // ------------------------------------------------------------ messages

    pub(crate) fn handle(&mut self, msg: ScriptIn) -> bool {
        let go = self.ingest(msg);
        if go {
            self.turn();
        }
        go
    }

    pub(crate) fn ingest(&mut self, msg: ScriptIn) -> bool {
        match msg {
            ScriptIn::SetInstances(specs) => self.set_instances(specs),
            ScriptIn::FileChanged { rel_path, source } => self.file_changed(&rel_path, source),
            ScriptIn::Hub(HubEvent::Service(ev)) => self.service_event(ev),
            ScriptIn::Hub(HubEvent::Io(ev)) => self.io_event(ev),
            // Images are the renderer's; the binary never routes them here.
            ScriptIn::Hub(HubEvent::Image { .. }) => {}
            ScriptIn::Element(call) => {
                self.sh.st.borrow_mut().ready.entry(call.instance).or_default().push_back(Work::Element(call));
            }
            ScriptIn::Popup { name, action, monitor, gesture } => self.popup_message(&name, action, monitor, gesture),
            ScriptIn::Input(call) => {
                self.sh.st.borrow_mut().ready.entry(call.instance).or_default().push_back(Work::Input(call));
            }
            ScriptIn::Trigger { event, data } => {
                let data = Rc::new(data);
                let mut st = self.sh.st.borrow_mut();
                let subs: Vec<(u64, InstanceId)> =
                    st.event_subs.iter().filter(|(_, e)| e.name == event).map(|(k, e)| (*k, e.inst)).collect();
                for (sub, inst) in subs {
                    st.ready.entry(inst).or_default().push_back(Work::Event { sub, data: Rc::clone(&data) });
                }
            }
            ScriptIn::Theme { dark } => self.set_theme(dark),
            ScriptIn::ClockChanged => self.clock_changed(),
            ScriptIn::Status { reply } => {
                let _ = reply.send(self.status());
            }
            ScriptIn::Shutdown => {
                self.shutdown();
                return false;
            }
        }
        true
    }

    fn shutdown(&mut self) {
        let ids: Vec<InstanceId> = self.sh.st.borrow().instances.keys().copied().collect();
        for id in ids {
            self.remove_instance(id);
        }
        self.vms.clear();
        self.vm_keys.clear();
    }

    // ------------------------------------------------------------ VMs

    fn total_memory(&self) -> usize {
        self.vms.values().map(|v| v.lua.used_memory()).sum()
    }

    fn ensure_vm(&mut self, key: &VmKey) -> Result<VmId, String> {
        if let Some(id) = self.vm_keys.get(key) {
            return Ok(*id);
        }
        let lim = self.sh.env.limits;
        let limit = if *key == VmKey::Builtin { lim.shared_vm_memory } else { lim.user_vm_memory };
        if *key != VmKey::Builtin && self.total_memory() + limit.min(1 << 20) >= lim.total_vm_memory {
            return Err(format!("widgets already use {} MB; this one cannot load", lim.total_vm_memory >> 20));
        }
        let dark = self.sh.st.borrow().dark;
        let name = match key {
            VmKey::Builtin => None,
            VmKey::User(rel) => rel.strip_prefix("widgets/").and_then(|r| r.strip_suffix(".luau")),
        };
        let stall = crate::test_stall_enabled_for(name);
        let vm = vm::create(&self.sh, &self.prelude, limit, dark, stall)
            .map_err(|e| format!("cannot start the script engine: {}", error_text(&e)))?;
        self.next_vm += 1;
        let id = self.next_vm;
        self.vms.insert(id, vm);
        self.vm_keys.insert(key.clone(), id);
        // Seed it with what the services already said.
        let events: Vec<ServiceEvent> = self.sh.st.borrow().svc.values().cloned().collect();
        for ev in events {
            self.push_service(id, &ev);
        }
        Ok(id)
    }

    fn drop_unused_vms(&mut self) {
        let used: std::collections::HashSet<VmKey> = self.vm_of_inst.values().cloned().collect();
        let stale: Vec<VmKey> =
            self.vm_keys.keys().filter(|k| **k != VmKey::Builtin && !used.contains(*k)).cloned().collect();
        for k in stale {
            if let Some(id) = self.vm_keys.remove(&k) {
                self.vms.remove(&id);
                self.sh.st.borrow_mut().vm_mem.remove(&id);
            }
        }
    }

    // ------------------------------------------------------------ instances

    fn set_instances(&mut self, specs: Vec<InstanceSpec>) {
        let want: HashMap<InstanceId, &InstanceSpec> = specs.iter().map(|s| (s.instance, s)).collect();
        let have: Vec<InstanceId> = self.sh.st.borrow().instances.keys().copied().collect();
        for id in have {
            let keep = match (want.get(&id), self.sh.st.borrow().instances.get(&id)) {
                (Some(w), Some(i)) => i.spec.same_identity(w),
                _ => false,
            };
            if !keep {
                self.remove_instance(id);
            }
        }
        self.sh.st.borrow_mut().plates_shown.retain(|id| want.contains_key(id));
        for spec in specs {
            let id = spec.instance;
            let scale_change = {
                let mut st = self.sh.st.borrow_mut();
                match st.instances.get_mut(&id) {
                    Some(i) => {
                        let changed = (i.spec.monitor.scale - spec.monitor.scale).abs() > f32::EPSILON;
                        i.spec = spec.clone();
                        Some(changed)
                    }
                    None => None,
                }
            };
            match scale_change {
                Some(true) => {
                    let scale = f64::from(spec.monitor.scale);
                    self.call_in(id, "update", |_, e| e.update_monitor.call::<()>((id.0, scale)));
                }
                Some(false) => {}
                None => self.start_instance(spec, None),
            }
        }
    }

    fn resolve_entry(&self, widget: &WidgetRef) -> Result<Resolved, String> {
        for rel in widget_files(widget) {
            if let Some(src) = self.sh.user_source(&rel)? {
                let replaces_builtin = !widget.is_path()
                    && self.sh.env.sources.builtin(&format!("widgets/{}.luau", widget.name)).is_some();
                if replaces_builtin && self.sh.st.borrow_mut().replaced_reported.insert(rel.clone()) {
                    log::warn!("{rel} replaces the built-in {} widget, with its popups and hotkey", widget.name);
                }
                return Ok(Resolved { rel, src, builtin: false, replaces_builtin });
            }
        }
        if !widget.is_path() {
            let rel = format!("widgets/{}.luau", widget.name);
            if let Some(src) = self.sh.env.sources.builtin(&rel) {
                return Ok(Resolved { rel, src: src.into_owned(), builtin: true, replaces_builtin: false });
            }
        }
        let mut names = self.sh.env.sources.builtin_widgets();
        names.sort();
        let near = suggest(&widget.name, &names);
        Err(match near {
            Some(n) => format!("unknown widget '{}'; did you mean '{n}'?", widget.name),
            None => format!("unknown widget '{}'", widget.name),
        })
    }

    fn new_inst(&self, spec: InstanceSpec, r: &Resolved, carry: Option<(u32, Duration)>) -> Inst {
        let now = self.sh.mono();
        let (restarts, backoff) = carry.unwrap_or((0, Duration::ZERO));
        let entry = if r.builtin { "widgets".to_string() } else { parent_dir(&r.rel) };
        Inst {
            id: spec.instance,
            ref_text: spec.widget.to_string(),
            source: if r.builtin { WidgetSource::Builtin } else { WidgetSource::User(r.rel.clone()) },
            builtin: r.builtin,
            replaces_builtin: r.replaces_builtin,
            files: std::iter::once(r.rel.clone()).collect(),
            entry,
            perms: Permissions::default(),
            grant: None,
            state: WidgetState::Loading,
            plate: None,
            calls: 0,
            script_time: Duration::ZERO,
            script_time_max: Duration::ZERO,
            errors: 0,
            restarts,
            backoff,
            restart_at: None,
            started_at: now,
            version: 0,
            dirty: false,
            error_until: None,
            last_error: None,
            err_times: Default::default(),
            cpu: Default::default(),
            cpu_sum: Duration::ZERO,
            log_win: Window::new(),
            log_reported: None,
            event_win: Window::new(),
            event_bytes_win: Window::new(),
            next_handler: 0,
            handlers: HashMap::new(),
            subs: Default::default(),
            modules: HashMap::new(),
            loading: Vec::new(),
            timer_warned: false,
            widget_name: String::new(),
            gesture_cmd_at: None,
            spec,
            vm: 0,
        }
    }

    fn start_instance(&mut self, spec: InstanceSpec, carry: Option<(u32, Duration)>) {
        let id = spec.instance;
        let resolved = match self.resolve_entry(&spec.widget) {
            Ok(r) => r,
            Err(msg) => {
                let r = Resolved { rel: String::new(), src: String::new(), builtin: false, replaces_builtin: false };
                let inst = self.new_inst(spec.clone(), &r, carry);
                self.sh.st.borrow_mut().instances.insert(id, inst);
                let diag = Diagnostic::error(DiagKind::Config, spec.pos.clone(), msg);
                self.fail(id, Why::Diag(diag));
                return;
            }
        };
        let key = if resolved.builtin { VmKey::Builtin } else { VmKey::User(resolved.rel.clone()) };
        let mut inst = self.new_inst(spec.clone(), &resolved, carry);
        let vm_id = match self.ensure_vm(&key) {
            Ok(v) => v,
            Err(msg) => {
                self.sh.st.borrow_mut().instances.insert(id, inst);
                self.fail(id, Why::Diag(Diagnostic::error(DiagKind::Stopped, None, format!("{}: {msg}", spec.widget))));
                return;
            }
        };
        inst.vm = vm_id;
        self.sh.st.borrow_mut().instances.insert(id, inst);
        self.vm_of_inst.insert(id, key);
        self.load_instance(id, &resolved);
    }

    fn load_instance(&mut self, id: InstanceId, r: &Resolved) {
        let sh = self.sh.clone();
        let (vm_id, spec) = {
            let st = sh.st.borrow();
            let Some(i) = st.instances.get(&id) else { return };
            (i.vm, i.spec.clone())
        };
        let Some(vm) = self.vms.get(&vm_id) else { return };
        let lim = sh.env.limits;
        let chunk = if r.builtin {
            format!("builtin/{}", r.rel.strip_prefix("widgets/").unwrap_or(&r.rel))
        } else {
            r.rel.clone()
        };
        let src = r.src.clone();
        let builtin = r.builtin;
        // A file directly in widgets\ has no folder of its own (it would be everyone's).
        let dir = if r.builtin || parent_dir(&r.rel) == rsb_core::paths::WIDGETS_DIR {
            String::new()
        } else {
            parent_dir(&r.rel)
        };
        let outcome = sh.guarded(
            Call {
                inst: Some(id),
                label: "load",
                budget: lim.setup_budget,
                loading: true,
                mem: (vm.limit, vm.soft),
                reset: Some(&vm.entry.reset),
            },
            || {
                let lua = &vm.lua;
                let entry = &vm.entry;
                let load = entry.load.clone();
                let chunk_name = format!("={chunk}");
                let body = lua.create_function(move |lua, ()| {
                    let f = lua.load(src.as_str()).set_name(chunk_name.clone()).into_function()?;
                    load.call::<Lv>(f)
                })?;
                let thread = lua.create_thread(body)?;
                thread.sandbox()?;
                let handle: Lv = thread.resume(())?;
                if thread.status() != mlua::prelude::LuaThreadStatus::Finished {
                    return Err(mlua::Error::runtime("a widget file cannot wait or yield while it loads"));
                }
                let spec_parts: MultiValue = entry.widget_spec.call(handle.clone())?;
                let mut it = spec_parts.into_iter();
                let Some(Lv::String(name)) = it.next() else {
                    return Err(mlua::Error::runtime("a widget file must return rsb.widget { ... }"));
                };
                let defaults_lv = it.next().unwrap_or(Lv::Nil);
                let optional_lv = it.next().unwrap_or(Lv::Nil);
                let perms_lv = it.next().unwrap_or(Lv::Nil);
                let check = || sh.check_limits(Some(lua));
                let defaults = from_lua(&defaults_lv, &mut Walk::new(&check))?;
                let optional = match from_lua(&optional_lv, &mut Walk::new(&check))? {
                    Value::List(l) => l.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let declared = from_lua(&perms_lv, &mut Walk::new(&check))?;
                check_permission_keys(&declared)?;
                let mut perms = Permissions::from_value(&declared).map_err(mlua::Error::runtime)?;
                if !builtin {
                    perms.grant_user_defaults();
                }
                let (perms, notes) =
                    perms.with_options(&sh.env.path_env, &|k| spec.options.values.get(k).map(|s| s.value.clone()));
                let widget_dir =
                    if dir.is_empty() { PathBuf::new() } else { sh.env.config_dir.join(dir.replace('/', "\\")) };
                let grant = Arc::new(IoGrant {
                    widget: spec.widget.to_string(),
                    permissions: perms.clone(),
                    widget_dir,
                    env: sh.env.path_env.clone(),
                });
                let widget_name = name.to_string_lossy();
                let mut diags = Vec::new();
                let props = build_props(lua, &spec, &defaults, &optional, &mut diags)?;
                for d in &diags {
                    log::warn!("{d}");
                }
                {
                    let mut st = sh.st.borrow_mut();
                    st.diags.extend(diags);
                    if let Some(i) = st.instances.get_mut(&id) {
                        i.perms = perms.clone();
                        i.grant = Some(grant);
                        i.widget_name = widget_name.clone();
                    }
                }
                log::info!("granted {}: {}", spec.widget, perms.describe_in(Some(&sh.env.path_env)));
                for n in notes {
                    log::info!("{}: {n}", spec.widget);
                }
                let ctx = build_ctx(lua, &spec, &widget_name, &chunk_dir(&chunk))?;
                entry.start.call::<()>((id.0, handle, props, ctx))?;
                Ok(())
            },
        );
        match outcome {
            Outcome::Ok(()) => {
                let now = sh.mono();
                let mut st = sh.st.borrow_mut();
                if let Some(i) = st.instances.get_mut(&id) {
                    i.state = WidgetState::Running;
                    i.started_at = now;
                    i.dirty = true;
                }
                if st.plates_shown.remove(&id) {
                    st.plates.push((id, None));
                }
                drop(st);
                self.collect_pending();
            }
            Outcome::Err(msg) => self.fail(id, Why::Script(msg)),
            Outcome::Stopped(s) => self.fail(id, Why::Stop(s)),
        }
    }

    /// Stops an instance: plate, log line, teardown, and a restart schedule for
    /// built-ins.
    fn fail(&mut self, id: InstanceId, why: Why) {
        let (name, builtin, vm) = {
            let st = self.sh.st.borrow();
            let Some(i) = st.instances.get(&id) else { return };
            (i.ref_text.clone(), i.builtin, i.vm)
        };
        let lim = self.sh.env.limits;
        let now = self.sh.mono();
        let (diag, restartable) = match why {
            Why::Stop(s) => {
                let d = s.diagnostic(&name, builtin);
                log::warn!("{}", s.log_line(&name));
                let restart = !matches!(s, Stop::Permission(_) | Stop::UpdateLoop);
                let restart = restart || builtin;
                (d, restart && builtin)
            }
            Why::Script(msg) => {
                let (pos, text) = split_pos(&msg);
                // Like the STOPPED plates: say what happens next. A built-in
                // starts again by itself and has only its settings to fix.
                let tail = if builtin { "restarting - check its settings in bar.toml" } else { "fix it and save" };
                let d = Diagnostic::error(DiagKind::Script, pos, crate::shared::with_tail(&text, tail));
                log::error!("{name}: {}", crate::shared::escape_log(&d.to_string(), 400));
                (d, builtin)
            }
            Why::Diag(d) => {
                log::warn!("{name}: {d}");
                (d, false)
            }
        };
        self.teardown(id);
        let mut mem_rebuild = false;
        {
            let mut st = self.sh.st.borrow_mut();
            if let Some(i) = st.instances.get_mut(&id) {
                let ran = now.saturating_duration_since(i.started_at);
                let text = diag.plate_text();
                i.state = WidgetState::Stopped(text.clone());
                i.plate = Some(Plate { kind: diag.kind, text, more: 0 });
                i.dirty = false;
                if restartable {
                    let next = if i.backoff.is_zero() || ran > Duration::from_secs(60) {
                        lim.builtin_restart_min
                    } else {
                        (i.backoff * 2).min(lim.builtin_restart_max)
                    };
                    i.backoff = next;
                    i.restart_at = Some(now + next);
                    log::info!("restarting {name} in {} s", next.as_secs());
                } else {
                    i.restart_at = None;
                }
                let plate = i.plate.clone();
                st.plates.push((id, plate));
                st.cleared.push(id);
                st.plates_shown.insert(id);
            }
            if builtin && let Some(v) = self.vms.get(&vm) {
                // The stopped instance's garbage goes first: only memory that
                // survives a full collection counts.
                let _ = v.lua.gc_collect();
                mem_rebuild = v.lua.used_memory() as f64 > 0.9 * v.limit as f64;
            }
        }
        if mem_rebuild {
            self.rebuild_vm(vm);
        }
    }

    /// Everything the instance owns on the Rust side, and its Luau scopes.
    fn teardown(&mut self, id: InstanceId) {
        let sh = self.sh.clone();
        {
            let mut st = sh.st.borrow_mut();
            st.tasks.retain(|_, t| t.inst != id);
            st.timers.retain(|_, t| t.inst != id);
            st.io.retain(|_, (i, _)| *i != id);
            st.event_subs.retain(|_, e| e.inst != id);
            st.ready.remove(&id);
            st.deferred.retain(|(i, _)| *i != id);
            st.pending_flush.remove(&id);
            let popups: Vec<u64> = st.popups.iter().filter(|(_, r)| r.inst == id).map(|(pid, _)| *pid).collect();
            for pid in popups {
                st.popup_drop(pid);
            }
            if let Some(i) = st.instances.get_mut(&id) {
                i.handlers.clear();
                i.subs.clear();
                i.modules.clear();
            }
        }
        sh.hub.send(HubRequest::CancelOwner { owner: id });
        // Luau side: cleanups run, then everything is dropped. A failure
        // here only means some cleanup did not run.
        let vm_id = sh.st.borrow().instances.get(&id).map(|i| i.vm);
        if let Some(vm) = vm_id.and_then(|v| self.vms.get(&v)) {
            let lim = sh.env.limits;
            let _ = sh.guarded(
                Call {
                    inst: Some(id),
                    label: "cleanup",
                    budget: lim.setup_budget,
                    loading: false,
                    mem: (vm.limit, vm.soft),
                    reset: Some(&vm.entry.reset),
                },
                || vm.entry.dispose.call::<()>(id.0),
            );
        }
    }

    fn remove_instance(&mut self, id: InstanceId) {
        self.teardown(id);
        {
            let mut st = self.sh.st.borrow_mut();
            if st.instances.get(&id).is_some_and(|i| i.error_until.is_some()) {
                st.error_notes.push((id, None));
            }
            st.instances.remove(&id);
        }
        self.vm_of_inst.remove(&id);
        self.drop_unused_vms();
    }

    fn rebuild_vm(&mut self, vm_id: VmId) {
        let key = self.vm_keys.iter().find(|(_, v)| **v == vm_id).map(|(k, _)| k.clone());
        let Some(key) = key else { return };
        let members: Vec<(InstanceId, InstanceSpec, u32, Duration)> = {
            let st = self.sh.st.borrow();
            st.instances
                .values()
                // A stopped instance keeps its plate and its restart schedule.
                .filter(|i| i.vm == vm_id && !matches!(i.state, WidgetState::Stopped(_)))
                .map(|i| (i.id, i.spec.clone(), i.restarts, i.backoff))
                .collect()
        };
        log::warn!("rebuilding the shared widget VM: memory stayed above 90% after a full collection");
        for (id, ..) in &members {
            self.teardown(*id);
            {
                let mut st = self.sh.st.borrow_mut();
                if st.instances.get(id).is_some_and(|i| i.error_until.is_some()) {
                    st.error_notes.push((*id, None));
                }
                st.instances.remove(id);
            }
            self.vm_of_inst.remove(id);
        }
        self.vm_keys.remove(&key);
        self.vms.remove(&vm_id);
        for (_, spec, restarts, backoff) in members {
            self.start_instance(spec, Some((restarts, backoff)));
        }
    }

    fn file_changed(&mut self, rel: &str, source: Option<String>) {
        let is_luau = std::path::Path::new(rel).extension().is_some_and(|e| e.eq_ignore_ascii_case("luau"));
        if !rel.starts_with("widgets/") || !is_luau {
            return;
        }
        // Widget files are keyed with a lower-case extension, whatever the file is called.
        let rel = format!("{}.luau", &rel[..rel.len() - ".luau".len()]);
        let rel = rel.as_str();
        self.sh.st.borrow_mut().user_src.insert(rel.to_string(), source);
        let affected: Vec<InstanceId> = {
            let st = self.sh.st.borrow();
            st.instances
                .values()
                .filter(|i| i.files.contains(rel) || widget_files(&i.spec.widget).iter().any(|f| f == rel))
                .map(|i| i.id)
                .collect()
        };
        for id in affected {
            let carry;
            let spec;
            {
                let st = self.sh.st.borrow();
                let Some(i) = st.instances.get(&id) else { continue };
                spec = i.spec.clone();
                carry = Some((i.restarts, i.backoff));
            }
            self.remove_instance(id);
            self.start_instance(spec, carry);
        }
    }

    fn set_theme(&mut self, dark: bool) {
        self.sh.st.borrow_mut().dark = dark;
        let lim = self.sh.env.limits;
        let ids: Vec<VmId> = self.vms.keys().copied().collect();
        for vm_id in ids {
            let Some(vm) = self.vms.get(&vm_id) else { continue };
            let _ = self.sh.guarded(
                Call {
                    inst: None,
                    label: "theme",
                    budget: lim.callback_budget,
                    loading: false,
                    mem: (vm.limit, vm.soft),
                    reset: Some(&vm.entry.reset),
                },
                || vm.entry.set_theme.call::<()>(dark),
            );
        }
        self.collect_pending();
    }

    fn clock_changed(&mut self) {
        let now = self.sh.mono();
        let mut st = self.sh.st.borrow_mut();
        let clock = self.sh.env.clock.clone();
        for t in st.timers.values_mut() {
            match &t.kind {
                TimerKind::Every { period, aligned: true } => {
                    let ms = period.as_millis();
                    let l = clock.now(TimeZone::Local);
                    let into =
                        (u128::from(l.minute) * 60_000 + u128::from(l.second) * 1000 + u128::from(l.millis)) % ms;
                    t.next = Some(now + Duration::from_millis((ms - into) as u64));
                }
                TimerKind::Clock { .. } => t.next = Some(now),
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------ services and I/O

    fn push_service(&mut self, vm_id: VmId, ev: &ServiceEvent) {
        let Some(vm) = self.vms.get(&vm_id) else { return };
        let sh = self.sh.clone();
        let lim = sh.env.limits;
        let outcome = sh.guarded(
            Call {
                inst: None,
                label: "service",
                budget: lim.callback_budget,
                loading: false,
                mem: (vm.limit, vm.soft),
                reset: Some(&vm.entry.reset),
            },
            || {
                let lua = &vm.lua;
                let t = lua.create_table()?;
                t.set("status", ev.status.name())?;
                t.set("stale", ev.status == ServiceStatus::Stale)?;
                t.set("error", ev.error.clone())?;
                if let Some(v) = &ev.value {
                    t.set("value", to_lua(lua, v, true)?)?;
                }
                vm.entry.svc_event.call::<()>((ev.service.name(), t))
            },
        );
        if let Outcome::Err(m) | Outcome::Stopped(Stop::Permission(m)) = outcome {
            log::warn!("service {} update failed: {m}", ev.service.name());
        }
    }

    fn service_event(&mut self, ev: ServiceEvent) {
        self.sh.st.borrow_mut().svc.insert(ev.service, ev.clone());
        let ids: Vec<VmId> = self.vms.keys().copied().collect();
        for id in ids {
            self.push_service(id, &ev);
        }
        self.collect_pending();
    }

    fn io_event(&mut self, ev: IoEvent) {
        let sh = self.sh.clone();
        let mut st = sh.st.borrow_mut();
        let Some((owner, wait)) = st.io.get(&ev.id).map(|(o, w)| {
            let tag = match w {
                IoWait::Task { task, kind } => (Some((*task, *kind)), None, None),
                IoWait::Spawn { on_line, on_exit } => (None, Some((on_line.clone(), on_exit.clone())), None),
                IoWait::Watch { cb } => (None, None, Some(cb.clone())),
                IoWait::Open => (None, None, None),
            };
            (*o, tag)
        }) else {
            return;
        };
        let denied = match &ev.kind {
            IoEventKind::ExecDone(Err(rsb_core::io::IoError::NotPermitted(n)))
            | IoEventKind::Http(Err(rsb_core::io::IoError::NotPermitted(n)))
            | IoEventKind::File(Err(rsb_core::io::IoError::NotPermitted(n))) => Some(n.clone()),
            _ => None,
        };
        if let Some(need) = denied {
            drop(st);
            self.fail(owner, Why::Stop(Stop::Permission(need)));
            return;
        }
        match wait {
            (Some((task, _kind)), _, _) => {
                st.io.remove(&ev.id);
                let payload = match ev.kind {
                    IoEventKind::ExecDone(r) => Payload::Exec(r),
                    IoEventKind::Http(r) => Payload::Http(r),
                    IoEventKind::File(r) => Payload::File(r),
                    _ => Payload::Nothing,
                };
                if !matches!(payload, Payload::Nothing) {
                    st.ready.entry(owner).or_default().push_back(Work::Resume { task, payload });
                }
            }
            (_, Some((on_line, on_exit)), _) => match ev.kind {
                IoEventKind::ExecLine { stderr, line } => {
                    if let Some(f) = on_line {
                        let stream = if stderr { "stderr" } else { "stdout" };
                        st.ready
                            .entry(owner)
                            .or_default()
                            .push_back(Work::Callback { f, args: vec![Value::Str(line), Value::Str(stream.into())] });
                    }
                }
                IoEventKind::ExecDone(r) => {
                    st.io.remove(&ev.id);
                    let code = match &r {
                        Ok(res) => res.code.map_or(Value::Nil, |c| Value::Int(i64::from(c))),
                        Err(_) => Value::Nil,
                    };
                    if let Err(e) = &r {
                        drop(st);
                        sh.script_log(Some(owner), log::Level::Warn, &format!("process ended: {e}"));
                        st = sh.st.borrow_mut();
                    }
                    if let Some(f) = on_exit {
                        st.ready.entry(owner).or_default().push_back(Work::Callback { f, args: vec![code] });
                    }
                }
                _ => {}
            },
            (_, _, Some(cb)) => {
                if let IoEventKind::File(r) = ev.kind {
                    let args = match r {
                        Ok(text) => vec![Value::Str(text), Value::Nil],
                        Err(e) => vec![Value::Nil, Value::Str(e.to_string())],
                    };
                    st.ready.entry(owner).or_default().push_back(Work::Callback { f: cb, args });
                }
            }
            _ => {
                st.io.remove(&ev.id);
                if let IoEventKind::Opened(Err(e)) = ev.kind {
                    drop(st);
                    sh.script_log(Some(owner), log::Level::Warn, &format!("could not open the address: {e}"));
                }
            }
        }
    }

    // ------------------------------------------------------------ calls

    /// Runs `f` in the instance's VM as one guarded call. A stop or an
    /// uncaught error is handled here; the return is None then.
    fn call_in<R>(
        &mut self,
        id: InstanceId,
        label: &'static str,
        f: impl FnOnce(&Lua, &crate::vm::Entry) -> mlua::Result<R>,
    ) -> Option<R> {
        let sh = self.sh.clone();
        let vm_id = sh.st.borrow().instances.get(&id)?.vm;
        let outcome = {
            let vm = self.vms.get(&vm_id)?;
            let budget = sh.env.limits.callback_budget;
            sh.guarded(
                Call {
                    inst: Some(id),
                    label,
                    budget,
                    loading: false,
                    mem: (vm.limit, vm.soft),
                    reset: Some(&vm.entry.reset),
                },
                || f(&vm.lua, &vm.entry),
            )
        };
        let out = match outcome {
            Outcome::Ok(r) => Some(r),
            Outcome::Stopped(s) => {
                self.fail(id, Why::Stop(s));
                None
            }
            Outcome::Err(msg) => {
                if let Some(stop) = sh.record_error(Some(id), &msg) {
                    self.fail(id, Why::Stop(stop));
                }
                None
            }
        };
        if out.is_some() {
            self.check_memory(id, vm_id);
        }
        out
    }

    fn check_memory(&mut self, id: InstanceId, vm_id: VmId) {
        let lim = self.sh.env.limits;
        let Some(vm) = self.vms.get(&vm_id) else { return };
        let used = vm.lua.used_memory();
        self.sh.st.borrow_mut().vm_mem.insert(vm_id, used);
        if used > (8 << 20) && self.total_memory() > lim.total_vm_memory {
            self.fail(id, Why::Stop(Stop::Memory { limit: lim.total_vm_memory }));
        }
    }

    fn collect_pending(&mut self) {
        let mut st = self.sh.st.borrow_mut();
        let ids: Vec<InstanceId> = std::mem::take(&mut st.pending_flush).into_iter().collect();
        for id in ids {
            if !st.instances.contains_key(&id) {
                continue;
            }
            // Effects settle right after the call that woke them, before
            // that instance's next queued call.
            let q = st.ready.entry(id).or_default();
            if !matches!(q.front(), Some(Work::Flush)) {
                q.push_front(Work::Flush);
            }
        }
    }

    fn run_work(&mut self, id: InstanceId, work: Work) {
        let running = matches!(self.sh.st.borrow().instances.get(&id).map(|i| &i.state), Some(WidgetState::Running));
        if !running {
            return;
        }
        let sh = self.sh.clone();
        match work {
            Work::Flush => {
                let turn = sh.st.borrow().turn;
                self.call_in(id, "effect", |_, e| e.flush.call::<()>((id.0, turn as i64)));
            }
            Work::Timer(tid) => {
                let cb = {
                    let mut st = sh.st.borrow_mut();
                    let one_shot = matches!(st.timers.get(&tid).map(|t| &t.kind), Some(TimerKind::After));
                    let cb = st.timers.get(&tid).map(|t| t.cb.clone());
                    if one_shot {
                        st.timers.remove(&tid);
                    }
                    cb
                };
                if let Some(cb) = cb {
                    self.call_in(id, "timer", |lua, _| sh.spawn_task(lua, id, cb, MultiValue::new()).map(|_| ()));
                }
            }
            Work::Resume { task, payload } => {
                self.call_in(id, "task", |lua, _| {
                    let args = payload_args(lua, payload)?;
                    sh.resume_task(task, args)
                });
            }
            Work::Event { sub, data } => {
                let cb = sh.st.borrow().event_subs.get(&sub).map(|e| e.cb.clone());
                if let Some(cb) = cb {
                    self.call_in(id, "event", |lua, _| {
                        let args = MultiValue::from_vec(vec![to_lua(lua, &data, false)?]);
                        sh.spawn_task(lua, id, cb, args).map(|_| ())
                    });
                }
            }
            Work::Callback { f, args } => {
                self.call_in(id, "callback", |lua, _| {
                    let a = pop_args(lua, &args)?;
                    sh.spawn_task(lua, id, f, a).map(|_| ())
                });
            }
            Work::Input(call) => self.run_input(id, call),
            Work::Element(call) => self.run_element(id, call),
            Work::Popup { pid, action, name, gesture } => {
                let vm = self.vm_of(id);
                self.set_gesture(vm, gesture);
                self.call_in(id, "popup", |_, e| e.popup_cmd.call::<()>((id.0, pid, action.name(), name)));
                self.set_gesture(vm, None);
            }
        }
    }

    fn vm_of(&self, id: InstanceId) -> Option<VmId> {
        self.sh.st.borrow().instances.get(&id).map(|i| i.vm)
    }

    /// Marks the user input being handled: the id the commands that need a click or key look for, and the same id
    /// where the prelude can read it when a signal is written.
    fn set_gesture(&self, vm: Option<VmId>, gesture: Option<u64>) {
        self.sh.st.borrow_mut().gesture = gesture;
        if let Some(vm) = vm.and_then(|v| self.vms.get(&v)) {
            let _ = vm.n.raw_set("g", gesture.map_or(Lv::Nil, |g| Lv::Integer(g as i64)));
        }
    }

    /// A key, text, value, focus, scroll or popup event from a scene for one handler of the instance.
    fn run_element(&mut self, id: InstanceId, call: ElementCall) {
        let sh = self.sh.clone();
        let f = sh.st.borrow().instances.get(&id).and_then(|i| i.handlers.get(&call.handler.0).cloned());
        let Some(f) = f else { return };
        let (label, gesture) = match &call.event {
            ElementEvent::Key(e) => {
                if !e.down {
                    return;
                }
                ("on_key", (call.gesture != 0).then_some(call.gesture))
            }
            ElementEvent::TextChanged(_) => ("on_change", None),
            ElementEvent::Submitted(_) => ("on_submit", None),
            ElementEvent::ValueChanged { .. } => ("on_change", None),
            ElementEvent::Focus => ("on_focus", None),
            ElementEvent::Blur => ("on_blur", None),
            ElementEvent::Scrolled { .. } => ("on_scroll", None),
            ElementEvent::PopupClosed(_) => ("on_close", None),
            ElementEvent::PopupOpened(_) => ("on_open", None),
        };
        let vm = self.vm_of(id);
        self.set_gesture(vm, gesture);
        self.call_in(id, label, |lua, _| {
            let args = match &call.event {
                ElementEvent::Key(e) => {
                    let t = lua.create_table()?;
                    t.set("key", e.key.name())?;
                    t.set("ctrl", e.mods.ctrl)?;
                    t.set("shift", e.mods.shift)?;
                    t.set("alt", e.mods.alt)?;
                    t.set("repeat", e.repeat)?;
                    MultiValue::from_vec(vec![Lv::Table(t)])
                }
                ElementEvent::TextChanged(text) | ElementEvent::Submitted(text) => {
                    MultiValue::from_vec(vec![Lv::String(lua.create_string(&**text)?)])
                }
                ElementEvent::ValueChanged { value, done } => {
                    MultiValue::from_vec(vec![Lv::Number(*value), Lv::Boolean(*done)])
                }
                ElementEvent::Focus | ElementEvent::Blur => MultiValue::new(),
                ElementEvent::Scrolled { x, y } => {
                    let t = lua.create_table()?;
                    t.set("x", *x)?;
                    t.set("y", *y)?;
                    MultiValue::from_vec(vec![Lv::Table(t)])
                }
                ElementEvent::PopupClosed(reason) => {
                    MultiValue::from_vec(vec![Lv::String(lua.create_string(reason.name())?)])
                }
                ElementEvent::PopupOpened(name) => MultiValue::from_vec(vec![Lv::String(lua.create_string(&**name)?)]),
            };
            sh.spawn_task(lua, id, f, args).map(|_| ())
        });
        self.set_gesture(vm, None);
    }

    /// `rsb popup <name> [action]`: queues the change for the instance whose popup answers to the name on the
    /// monitor (None = the primary one).
    fn popup_message(&mut self, name: &str, action: PopupAction, monitor: Option<u64>, gesture: u64) {
        let mut st = self.sh.st.borrow_mut();
        let monitor = monitor.or_else(|| {
            let primary = st.instances.values().find(|i| i.spec.monitor.primary);
            let first = st.instances.values().min_by_key(|i| i.spec.monitor.index);
            primary.or(first).map(|i| i.spec.monitor.handle)
        });
        let Some(monitor) = monitor else { return };
        let target = st
            .popups
            .iter()
            .filter(|(_, r)| r.monitor == monitor && r.names.iter().any(|n| n == name))
            .map(|(pid, r)| (*pid, r.inst, st.instances.get(&r.inst).is_some_and(|i| i.builtin)))
            .min_by_key(|(pid, _, builtin)| (!*builtin, *pid));
        let Some((pid, owner, _)) = target else {
            log::info!("rsb popup: no popup is named '{}'", crate::shared::escape_log(name, 40));
            return;
        };
        st.ready.entry(owner).or_default().push_back(Work::Popup {
            pid,
            action,
            name: name.to_string(),
            gesture: (gesture != 0).then_some(gesture),
        });
    }

    /// Tells every VM which named popups opened or closed, so `rsb.popups.is_open` updates across widgets.
    fn push_popup_events(&mut self) {
        let events = std::mem::take(&mut self.sh.st.borrow_mut().popup_events);
        if events.is_empty() {
            return;
        }
        self.sweep |= events.iter().any(|(_, open)| !open);
        let sh = self.sh.clone();
        let lim = sh.env.limits;
        let ids: Vec<VmId> = self.vms.keys().copied().collect();
        for vm_id in ids {
            let Some(vm) = self.vms.get(&vm_id) else { continue };
            let _ = sh.guarded(
                Call {
                    inst: None,
                    label: "popup",
                    budget: lim.callback_budget,
                    loading: false,
                    mem: (vm.limit, vm.soft),
                    reset: Some(&vm.entry.reset),
                },
                || {
                    for (key, open) in &events {
                        vm.entry.popup_event.call::<()>((key.as_str(), *open))?;
                    }
                    Ok(())
                },
            );
        }
        self.collect_pending();
    }

    fn run_input(&mut self, id: InstanceId, call: HandlerCall) {
        let sh = self.sh.clone();
        let f = sh.st.borrow().instances.get(&id).and_then(|i| i.handlers.get(&call.handler.0).cloned());
        let Some(f) = f else { return };
        let label = match call.kind {
            InputKind::Click(Button::Left) => "on_click",
            InputKind::Click(Button::Right) => "on_right_click",
            InputKind::Click(Button::Middle) => "on_middle_click",
            InputKind::Wheel(_) => "on_wheel",
            InputKind::Enter => "on_enter",
            InputKind::Leave => "on_leave",
        };
        let gesture = (matches!(call.kind, InputKind::Click(_)) && call.gesture != 0).then_some(call.gesture);
        let vm = self.vm_of(id);
        self.set_gesture(vm, gesture);
        self.call_in(id, label, |lua, _| {
            let args = match call.kind {
                InputKind::Click(b) => {
                    let e = lua.create_table()?;
                    e.set("button", b.name())?;
                    e.set("x", call.x)?;
                    e.set("y", call.y)?;
                    MultiValue::from_vec(vec![Lv::Table(e)])
                }
                InputKind::Wheel(d) => {
                    let e = lua.create_table()?;
                    e.set("delta", d)?;
                    e.set("x", call.x)?;
                    e.set("y", call.y)?;
                    MultiValue::from_vec(vec![Lv::Table(e)])
                }
                InputKind::Enter | InputKind::Leave => MultiValue::new(),
            };
            sh.spawn_task(lua, id, f, args).map(|_| ())
        });
        self.set_gesture(vm, None);
    }

    // ------------------------------------------------------------ the turn

    pub(crate) fn run_due(&mut self) {
        self.turn();
    }

    fn collect_due(&mut self) {
        let sh = self.sh.clone();
        let now = sh.mono();
        let mut restarts: Vec<InstanceId> = Vec::new();
        {
            let mut st = sh.st.borrow_mut();
            st.turn += 1;
            let deferred = std::mem::take(&mut st.deferred);
            for (inst, w) in deferred {
                st.ready.entry(inst).or_default().push_back(w);
            }
            let mut fire: Vec<(u64, InstanceId)> = Vec::new();
            for (id, t) in st.timers.iter_mut() {
                let Some(next) = t.next else { continue };
                if next > now {
                    continue;
                }
                fire.push((*id, t.inst));
                match &t.kind {
                    TimerKind::After => t.next = None,
                    TimerKind::Every { period, aligned } => {
                        t.next = Some(if *aligned {
                            let l = sh.env.clock.now(TimeZone::Local);
                            let ms = period.as_millis();
                            let into =
                                (u128::from(l.minute) * 60_000 + u128::from(l.second) * 1000 + u128::from(l.millis))
                                    % ms;
                            now + Duration::from_millis((ms - into) as u64)
                        } else {
                            let n = next + *period;
                            if n <= now { now + *period } else { n }
                        });
                    }
                    TimerKind::Clock { precision, zone } => {
                        t.next = Some(now + until_boundary(&sh.env.clock.now(*zone), *precision));
                    }
                }
            }
            fire.sort();
            for (tid, inst) in fire {
                st.ready.entry(inst).or_default().push_back(Work::Timer(tid));
            }
            let due_tasks: Vec<(u64, InstanceId)> = st
                .tasks
                .iter()
                .filter(|(_, t)| matches!(t.wait, Wait::Sleep(at) if at <= now))
                .map(|(k, t)| (*k, t.inst))
                .collect();
            for (task, inst) in due_tasks {
                if let Some(t) = st.tasks.get_mut(&task) {
                    t.wait = Wait::Turn;
                }
                st.ready.entry(inst).or_default().push_back(Work::Resume { task, payload: Payload::Nothing });
            }
            let mut ended: Vec<InstanceId> = Vec::new();
            for i in st.instances.values_mut() {
                if let Some(at) = i.restart_at
                    && at <= now
                {
                    restarts.push(i.id);
                }
                if let Some(u) = i.error_until
                    && u <= now
                {
                    i.error_until = None;
                    i.last_error = None;
                    i.dirty = true;
                    ended.push(i.id);
                }
            }
            st.error_notes.extend(ended.into_iter().map(|id| (id, None)));
        }
        for id in restarts {
            let carry;
            let spec;
            {
                let st = sh.st.borrow();
                let Some(i) = st.instances.get(&id) else { continue };
                spec = i.spec.clone();
                carry = (i.restarts + 1, i.backoff);
            }
            self.remove_instance(id);
            self.start_instance(spec, Some(carry));
        }
    }

    fn next_ready(&mut self) -> Option<(InstanceId, Work)> {
        let mut st = self.sh.st.borrow_mut();
        st.ready.retain(|_, q| !q.is_empty());
        let pick = match self.rr_last {
            Some(last) => {
                st.ready.range((std::ops::Bound::Excluded(last), std::ops::Bound::Unbounded)).next().map(|(k, _)| *k)
            }
            None => None,
        }
        .or_else(|| st.ready.keys().next().copied())?;
        let w = st.ready.get_mut(&pick)?.pop_front()?;
        self.rr_last = Some(pick);
        Some((pick, w))
    }

    pub(crate) fn turn(&mut self) {
        self.collect_due();
        let started = Instant::now();
        let budget = self.sh.env.limits.turn_budget;
        // A bare yield or an event emitted while running waits for the next
        // turn (it lands in `deferred`), so this loop ends.
        while let Some((id, work)) = self.next_ready() {
            self.run_work(id, work);
            self.collect_pending();
            self.push_popup_events();
            if started.elapsed() >= budget {
                break;
            }
        }
        self.settle();
        self.flush_out();
        if std::mem::take(&mut self.sweep) {
            // The incremental collector would hold a closed popup's tree for a long time on an idle bar.
            for vm in self.vms.values() {
                let _ = vm.lua.gc_collect();
            }
        }
    }

    /// Runs the effect passes still waiting at the front of a queue, so the
    /// trees cut at the end of a turn show everything that ran in it, even
    /// when the turn's time ran out with calls left over.
    fn settle(&mut self) {
        for _ in 0..4 {
            let ids: Vec<InstanceId> = self
                .sh
                .st
                .borrow()
                .ready
                .iter()
                .filter(|(_, q)| matches!(q.front(), Some(Work::Flush)))
                .map(|(k, _)| *k)
                .collect();
            if ids.is_empty() {
                return;
            }
            for id in ids {
                let popped = self.sh.st.borrow_mut().ready.get_mut(&id).and_then(|q| q.pop_front());
                if let Some(w) = popped {
                    self.run_work(id, w);
                }
                self.collect_pending();
                self.push_popup_events();
            }
        }
    }

    fn flush_out(&mut self) {
        self.push_popup_events();
        let sh = self.sh.clone();
        let now = sh.mono();
        let ids: Vec<InstanceId> = sh
            .st
            .borrow()
            .instances
            .values()
            .filter(|i| i.dirty && matches!(i.state, WidgetState::Running))
            .map(|i| i.id)
            .collect();
        let mut trees: Vec<Arc<WidgetTree>> = Vec::new();
        for id in ids {
            let error_class = sh.st.borrow().instances.get(&id).is_some_and(|i| i.error_until.is_some_and(|u| now < u));
            if let Some(root) = self.call_in(id, "render", |lua, e| {
                let _ = lua;
                let t: Option<Table> = e.root.call(id.0)?;
                match t {
                    Some(t) => Ok(Some(snapshot(&sh, id, &t, error_class)?)),
                    None => Ok(None),
                }
            }) {
                let mut st = sh.st.borrow_mut();
                if let Some(i) = st.instances.get_mut(&id) {
                    i.dirty = false;
                    i.version += 1;
                    trees.push(Arc::new(WidgetTree {
                        instance: id,
                        widget: i.spec.widget.clone(),
                        version: i.version,
                        root,
                        grant: i.grant.clone(),
                    }));
                }
            }
        }
        let (plates, cleared, diags, notes) = {
            let mut st = sh.st.borrow_mut();
            (
                std::mem::take(&mut st.plates),
                std::mem::take(&mut st.cleared),
                std::mem::take(&mut st.diags),
                std::mem::take(&mut st.error_notes),
            )
        };
        for (instance, note) in notes {
            sh.out.send(ScriptOut::ErrorNote { instance, note });
        }
        for id in cleared {
            let mut st = sh.st.borrow_mut();
            if let Some(i) = st.instances.get_mut(&id) {
                i.version += 1;
                trees.push(Arc::new(WidgetTree {
                    instance: id,
                    widget: i.spec.widget.clone(),
                    version: i.version,
                    root: None,
                    grant: i.grant.clone(),
                }));
            }
        }
        for (instance, plate) in plates {
            sh.out.send(ScriptOut::Plate { instance, plate });
        }
        if !trees.is_empty() {
            sh.out.send(ScriptOut::Trees(trees));
        }
        if !diags.is_empty() {
            sh.out.send(ScriptOut::Diagnostics(diags));
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let st = self.sh.st.borrow();
        let now = self.sh.mono();
        if !st.ready.values().all(|q| q.is_empty()) || !st.deferred.is_empty() || !st.pending_flush.is_empty() {
            return Some(now);
        }
        let mut best: Option<Instant> = None;
        let mut take = |t: Instant| best = Some(best.map_or(t, |b| b.min(t)));
        for t in st.timers.values() {
            if let Some(n) = t.next {
                take(n);
            }
        }
        for t in st.tasks.values() {
            if let Wait::Sleep(at) = t.wait {
                take(at);
            }
        }
        for i in st.instances.values() {
            if let Some(r) = i.restart_at {
                take(r);
            }
            if let Some(u) = i.error_until {
                take(u);
            }
        }
        best
    }

    fn status(&self) -> Vec<WidgetStatus> {
        let st = self.sh.st.borrow();
        // A user widget placed more than once (one bar per monitor) runs in one
        // VM, like the built-ins: its rows each report that VM's whole size.
        let shares_vm = |id: InstanceId| {
            self.vm_of_inst.get(&id).is_some_and(|key| self.vm_of_inst.values().filter(|k| *k == key).count() > 1)
        };
        st.instances
            .values()
            .map(|i| WidgetStatus {
                instance: i.id,
                widget: i.ref_text.clone(),
                bar: i.spec.bar.clone(),
                state: i.state.clone(),
                script_time: i.script_time,
                script_time_max: i.script_time_max,
                calls: i.calls,
                vm_bytes: self.vms.get(&i.vm).map_or(0, |v| v.lua.used_memory()),
                shared_vm: i.builtin || shares_vm(i.id),
                source: i.source.clone(),
                replaces_builtin: i.replaces_builtin,
                errors: i.errors,
                restarts: i.restarts,
            })
            .collect()
    }
}

/// `Permissions::from_value` reads only the keys that hold entries, so a
/// misspelled key with an empty list would pass in silence.
fn check_permission_keys(declared: &Value) -> mlua::Result<()> {
    const KEYS: [&str; 7] = ["exec", "file", "net", "control", "read", "overlay", "popups"];
    let Value::Map(map) = declared else { return Ok(()) };
    let names: Vec<String> = KEYS.iter().map(|k| k.to_string()).collect();
    for key in map.keys() {
        if !KEYS.contains(&key.as_str()) {
            let hint = suggest(key, &names).map(|n| format!("; did you mean '{n}'?")).unwrap_or_default();
            return Err(mlua::Error::runtime(format!("unknown permission '{key}'{hint}")));
        }
    }
    Ok(())
}

fn chunk_dir(chunk: &str) -> String {
    parent_dir(chunk)
}

fn payload_args(lua: &Lua, p: Payload) -> mlua::Result<MultiValue> {
    use rsb_core::io::IoError;
    Ok(match p {
        Payload::Nothing => MultiValue::new(),
        Payload::Exec(r) => {
            let t = lua.create_table()?;
            match r {
                Ok(res) => {
                    t.set("ok", res.code == Some(0))?;
                    t.set("code", res.code)?;
                    t.set("stdout", res.stdout)?;
                    t.set("stderr", res.stderr)?;
                    t.set("timed_out", res.timed_out)?;
                    t.set("truncated", res.truncated)?;
                }
                Err(e) => {
                    t.set("ok", false)?;
                    t.set("stdout", "")?;
                    t.set("stderr", "")?;
                    t.set("timed_out", matches!(e, IoError::Timeout))?;
                    t.set("truncated", false)?;
                    t.set("error", e.to_string())?;
                }
            }
            MultiValue::from_vec(vec![Lv::Table(t)])
        }
        Payload::Http(r) => {
            let t = lua.create_table()?;
            let headers = lua.create_table()?;
            match r {
                Ok(res) => {
                    t.set("ok", (200..300).contains(&res.status))?;
                    t.set("status", res.status)?;
                    t.set("body", lua.create_string(&res.body)?)?;
                    for (k, v) in &res.headers {
                        headers.set(k.as_str(), v.as_str())?;
                    }
                    t.set("from_cache", res.from_cache)?;
                }
                Err(e) => {
                    t.set("ok", false)?;
                    t.set("status", 0)?;
                    t.set("body", "")?;
                    t.set("from_cache", false)?;
                    t.set("error", e.to_string())?;
                }
            }
            t.set("headers", headers)?;
            MultiValue::from_vec(vec![Lv::Table(t)])
        }
        Payload::File(r) => match r {
            Ok(text) => MultiValue::from_vec(vec![Lv::String(lua.create_string(&text)?)]),
            Err(e) => MultiValue::from_vec(vec![Lv::Nil, Lv::String(lua.create_string(e.to_string())?)]),
        },
    })
}

/// Option defaults overlaid with what the user set in `bar.toml`; mistakes
/// are warnings positioned at the line in `bar.toml`.
fn build_props(
    lua: &Lua,
    spec: &InstanceSpec,
    defaults: &Value,
    optional: &[String],
    diags: &mut Vec<Diagnostic>,
) -> mlua::Result<Table> {
    let mut props: BTreeMap<String, Value> = match defaults {
        Value::Map(m) => m.clone(),
        _ => BTreeMap::new(),
    };
    let declared: Vec<String> = props.keys().cloned().chain(optional.iter().cloned()).collect();
    for (k, sp) in &spec.options.values {
        match defaults.get(k) {
            Some(d) => {
                if d.type_name() == sp.value.type_name() {
                    props.insert(k.clone(), sp.value.clone());
                } else {
                    diags.push(Diagnostic::warning(
                        DiagKind::Config,
                        Some(sp.pos.clone()),
                        format!(
                            "option '{k}' of widget '{}' must be a {}, got a {}",
                            spec.widget,
                            d.type_name(),
                            sp.value.type_name()
                        ),
                    ));
                }
            }
            None if optional.contains(k) => {
                props.insert(k.clone(), sp.value.clone());
            }
            None => {
                let near = suggest(k, &declared);
                let msg = match near {
                    Some(n) => format!("unknown option '{k}' for widget '{}'; did you mean '{n}'?", spec.widget),
                    None => format!("unknown option '{k}' for widget '{}'", spec.widget),
                };
                diags.push(Diagnostic::warning(DiagKind::Config, Some(sp.pos.clone()), msg));
            }
        }
    }
    match to_lua(lua, &Value::Map(props), false)? {
        Lv::Table(t) => Ok(t),
        _ => lua.create_table(),
    }
}

fn build_ctx(lua: &Lua, spec: &InstanceSpec, widget_name: &str, dir: &str) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("id", spec.widget.to_string())?;
    t.set("name", if widget_name.is_empty() { spec.widget.name.as_str() } else { widget_name })?;
    t.set("bar", spec.bar.as_str())?;
    let m = lua.create_table()?;
    m.set("index", spec.monitor.index)?;
    m.set("handle", spec.monitor.handle as f64)?;
    m.set("device", spec.monitor.device.as_str())?;
    m.set("primary", spec.monitor.primary)?;
    m.set("scale", spec.monitor.scale)?;
    t.set("monitor", m)?;
    t.set("dir", dir)?;
    Ok(t)
}
