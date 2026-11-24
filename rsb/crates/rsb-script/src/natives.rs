//! The native functions the prelude calls (`N` in `prelude.luau`). Each one
//! starts with `guard()`, so once an instance has been told to stop, a script
//! that caught the first error with `pcall` still cannot use the API. Denied
//! permissions, limits and bad arguments all end here.

use std::cell::RefCell;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use mlua::prelude::LuaThreadStatus as ThreadStatus;
use mlua::{Error, Function, Lua, MultiValue, Table, Value as Lv};
use rsb_core::duration::{from_seconds, parse_duration};
use rsb_core::element::InstanceId;
use rsb_core::io::{
    ExecSpec, FileRequest, HttpMethod, HttpRequest, IoRequest, ProgramRef, check_headers, limits as iol,
};
use rsb_core::msg::HubRequest;
use rsb_core::paths::resolve_require;
use rsb_core::permissions::{Need, check_path_text, check_program_name, parse_url};
use rsb_core::services::{AudioCommand, MediaCommand, ServiceCommand, ServiceId, WorkspaceCommand};
use rsb_core::time::{LocalTime, Precision, TimeZone, format_time, precision_of, until_boundary};
use rsb_core::value::Value;

use crate::convert::{Walk, from_lua, lua_type, to_lua};
use crate::shared::*;

/// Longest argument list `rsb.exec` takes; the walk over it has no deadline
/// check of its own.
const MAX_COMMAND_WORDS: usize = 1024;

pub(crate) struct ElementHandle(pub Table);

fn rt(msg: impl Into<String>) -> Error {
    Error::runtime(msg.into())
}

/// Edit distance of at most two, for "did you mean".
pub(crate) fn suggest(name: &str, candidates: &[String]) -> Option<String> {
    let mut best: Option<(usize, &String)> = None;
    for c in candidates {
        let d = edit_distance(name, c);
        if d <= 2 && d < name.chars().count().max(2) && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c.clone())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

pub(crate) fn duration_arg(v: &Lv, what: &str) -> mlua::Result<Duration> {
    match v {
        Lv::Integer(i) => from_seconds(*i as f64).map_err(|e| rt(format!("{what}: {e}"))),
        Lv::Number(n) => from_seconds(*n).map_err(|e| rt(format!("{what}: {e}"))),
        Lv::String(s) => parse_duration(&s.to_string_lossy()).map_err(|e| rt(format!("{what}: {e}"))),
        other => Err(rt(format!("{what}: expected a duration like 2 or \"500ms\", got {}", lua_type(other)))),
    }
}

fn opt_get(opts: &Option<Table>, key: &str) -> mlua::Result<Lv> {
    match opts {
        Some(t) => t.raw_get::<Lv>(key),
        None => Ok(Lv::Nil),
    }
}

fn opt_usize(opts: &Option<Table>, key: &str, default: usize, max: usize) -> mlua::Result<usize> {
    match opt_get(opts, key)? {
        Lv::Nil => Ok(default),
        Lv::Integer(i) if i >= 0 => Ok((i as usize).min(max)),
        Lv::Number(n) if n >= 0.0 => Ok((n as usize).min(max)),
        other => Err(rt(format!("{key} must be a number of bytes, got {}", lua_type(&other)))),
    }
}

fn aligned(d: Duration) -> bool {
    let ms = d.as_millis();
    ms >= 1000 && ms.is_multiple_of(1000) && 3_600_000_u128.is_multiple_of(ms)
}

fn next_aligned(sh: &Shared, d: Duration) -> Duration {
    let t = sh.env.clock.now(TimeZone::Local);
    let ms = d.as_millis();
    let into = (u128::from(t.minute) * 60_000 + u128::from(t.second) * 1000 + u128::from(t.millis)) % ms;
    Duration::from_millis((ms - into) as u64)
}

pub(crate) fn parse_zone(v: &Lv) -> mlua::Result<TimeZone> {
    match v {
        Lv::Nil => Ok(TimeZone::Local),
        Lv::String(s) => TimeZone::parse(&s.to_string_lossy()).map_err(rt),
        other => Err(rt(format!("timezone must be a string, got {}", lua_type(other)))),
    }
}

fn time_table(lua: &Lua, t: &LocalTime) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    out.set("year", t.year)?;
    out.set("month", t.month)?;
    out.set("day", t.day)?;
    out.set("weekday", t.weekday)?;
    out.set("hour", t.hour)?;
    out.set("minute", t.minute)?;
    out.set("second", t.second)?;
    out.set("millis", t.millis)?;
    out.set("utc_offset", t.utc_offset_min)?;
    Ok(out)
}

fn time_from_table(t: &Table) -> mlua::Result<LocalTime> {
    let n = |k: &str| -> mlua::Result<f64> {
        match t.raw_get::<Lv>(k)? {
            Lv::Integer(i) => Ok(i as f64),
            Lv::Number(x) => Ok(x),
            _ => Err(rt(format!("a time needs a numeric field '{k}'"))),
        }
    };
    Ok(LocalTime {
        year: n("year")? as i32,
        month: n("month")? as u8,
        day: n("day")? as u8,
        weekday: n("weekday")? as u8,
        hour: n("hour")? as u8,
        minute: n("minute")? as u8,
        second: n("second")? as u8,
        millis: n("millis")? as u16,
        utc_offset_min: n("utc_offset")? as i32,
    })
}

/// Shortest gap between two addresses opened by `rsb.open_url`.
const OPEN_URL_GAP: Duration = Duration::from_secs(2);

/// Most data one `rsb.events.emit` may carry, and most a widget may emit in a second.
const EVENT_MAX_BYTES: usize = 64 << 10;
const EVENT_BYTES_PER_SEC: usize = 1 << 20;

/// Rough size of a value as text: enough to cap it, not exact.
fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Num(_) => 8,
        Value::Str(s) => s.len() + 2,
        Value::List(l) => 2 + l.iter().map(|x| value_bytes(x) + 1).sum::<usize>(),
        Value::Map(m) => 2 + m.iter().map(|(k, x)| k.len() + 4 + value_bytes(x)).sum::<usize>(),
    }
}

fn is_abs_text(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\')
}

impl Shared {
    pub(crate) fn inst_or_err(&self) -> mlua::Result<InstanceId> {
        self.current_instance().ok_or_else(|| rt("this can only be used by a running widget"))
    }

    fn deny(&self, need: Need) -> Error {
        self.kill(Stop::Permission(need.folded(&self.env.path_env).to_string()))
    }

    fn grant(&self, inst: InstanceId) -> mlua::Result<std::sync::Arc<rsb_core::io::IoGrant>> {
        let st = self.st.borrow();
        st.instances
            .get(&inst)
            .and_then(|i| i.grant.clone())
            .ok_or_else(|| rt("this widget has no permissions loaded yet"))
    }

    /// Expands `~` and `%VAR%`, joins relative paths to the widget folder,
    /// and folds `.` and `..` lexically. The result still goes through
    /// `Permissions::check_file`.
    pub(crate) fn resolve_path(&self, inst: InstanceId, text: &str) -> mlua::Result<PathBuf> {
        let grant = self.grant(inst)?;
        let deny_path = |s: &str| self.deny(Need::File(s.replace('\\', "/")));
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(rt("a path cannot be empty or hold control characters"));
        }
        if text.starts_with("\\\\") || text.starts_with("//") || text.starts_with("\\/") || text.starts_with("/\\") {
            return Err(deny_path(text));
        }
        if text.starts_with(['/', '\\']) {
            return Err(rt("a path cannot start with / or \\: start it with ~, a drive like C:/ or a folder name"));
        }
        let env = &grant.env;
        let (first, rest) = match text.find(['/', '\\']) {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => (text, ""),
        };
        let (base, rest): (PathBuf, &str) = if first == "~" {
            (env.home.clone(), rest)
        } else if first.eq_ignore_ascii_case("%APPDATA%") {
            (env.appdata.clone(), rest)
        } else if first.eq_ignore_ascii_case("%LOCALAPPDATA%") {
            (env.local_appdata.clone(), rest)
        } else if first.eq_ignore_ascii_case("%TEMP%") {
            (env.temp.clone(), rest)
        } else if first.len() == 2 && first.ends_with(':') && first.as_bytes()[0].is_ascii_alphabetic() {
            (PathBuf::from(format!("{first}\\")), rest)
        } else if grant.widget_dir.as_os_str().is_empty() {
            return Err(rt(
                "this widget has no folder of its own: write the path from ~ or a drive, or move the file into widgets\\<name>\\",
            ));
        } else {
            (grant.widget_dir.clone(), text)
        };
        let mut out = Vec::new();
        fold(&mut out, &base, self, text)?;
        for seg in rest.split(['/', '\\']) {
            push_segment(&mut out, seg, text, self)?;
        }
        Ok(collect(&out))
    }

    fn check_file(&self, inst: InstanceId, path: &Path) -> mlua::Result<()> {
        let grant = self.grant(inst)?;
        grant.permissions.check_file(path, &grant.widget_dir, &grant.env).map_err(|n| self.deny(n))
    }

    /// Path argument for an I/O call: resolved and permitted.
    pub(crate) fn permitted_path(&self, inst: InstanceId, text: &str) -> mlua::Result<PathBuf> {
        let p = self.resolve_path(inst, text)?;
        self.check_file(inst, &p)?;
        Ok(p)
    }

    pub(crate) fn user_source(&self, rel: &str) -> Result<Option<String>, String> {
        if let Some(c) = self.st.borrow().user_src.get(rel) {
            return Ok(c.clone());
        }
        let got = self.env.sources.user(rel).map_err(|e| format!("cannot read {rel}: {}", e.kind()))?;
        self.st.borrow_mut().user_src.insert(rel.to_string(), got.clone());
        Ok(got)
    }

    pub(crate) fn spawn_task(&self, lua: &Lua, inst: InstanceId, f: Function, args: MultiValue) -> mlua::Result<u64> {
        let max = self.env.limits.max_tasks_per_instance;
        let id = {
            let mut st = self.st.borrow_mut();
            if st.tasks.values().filter(|t| t.inst == inst).count() >= max {
                return Err(rt(format!("too many tasks (at most {max} at once)")));
            }
            st.next_task += 1;
            st.next_task
        };
        let thread = lua.create_thread(f)?;
        self.st.borrow_mut().tasks.insert(id, Task { inst, thread, wait: Wait::Running });
        self.resume_task(id, args)?;
        Ok(id)
    }

    /// Resumes a task. Errors inside the task are the task's own (logged,
    /// counted); an `Err` here means the instance must stop.
    pub(crate) fn resume_task(&self, id: u64, args: MultiValue) -> mlua::Result<()> {
        let (thread, inst) = {
            let st = self.st.borrow();
            match st.tasks.get(&id) {
                Some(t) => (t.thread.clone(), t.inst),
                None => return Ok(()),
            }
        };
        if thread.status() != ThreadStatus::Resumable {
            self.st.borrow_mut().tasks.remove(&id);
            return Ok(());
        }
        let prev = {
            let mut st = self.st.borrow_mut();
            if let Some(t) = st.tasks.get_mut(&id) {
                t.wait = Wait::Running;
            }
            st.cur_task.replace(id)
        };
        let r = thread.resume::<MultiValue>(args);
        self.st.borrow_mut().cur_task = prev;
        match r {
            Ok(_) => {
                let alive = matches!(thread.status(), ThreadStatus::Resumable);
                let mut st = self.st.borrow_mut();
                if alive {
                    if let Some(t) = st.tasks.get_mut(&id)
                        && t.wait == Wait::Running
                    {
                        t.wait = Wait::Turn;
                        st.deferred.push((inst, Work::Resume { task: id, payload: Payload::Nothing }));
                    }
                } else {
                    st.tasks.remove(&id);
                }
                Ok(())
            }
            Err(e) => {
                self.st.borrow_mut().tasks.remove(&id);
                if self.bud.killed.get() {
                    return Err(stopped_error());
                }
                if is_memory_error(&e) {
                    return Err(self.kill(Stop::Memory { limit: self.bud.mem_limit.get() }));
                }
                let msg = error_text(&e);
                match self.record_error(Some(inst), &msg) {
                    Some(stop) => Err(self.kill(stop)),
                    None => Ok(()),
                }
            }
        }
    }

    /// Ids are counted across every widget, so a cancel only counts when it
    /// comes from the widget that owns the thing it names.
    fn cancel_task(&self, id: u64) {
        let me = self.current_instance();
        let mut st = self.st.borrow_mut();
        if !st.tasks.get(&id).is_some_and(|t| Some(t.inst) == me) {
            return;
        }
        st.tasks.remove(&id);
        let ios: Vec<rsb_core::io::IoId> = st
            .io
            .iter()
            .filter_map(|(k, (_, w))| match w {
                IoWait::Task { task, .. } if *task == id => Some(*k),
                _ => None,
            })
            .collect();
        for k in &ios {
            st.io.remove(k);
        }
        drop(st);
        for k in ios {
            self.hub.send(HubRequest::Cancel { id: k });
        }
    }

    fn cur_task(&self, what: &str) -> mlua::Result<u64> {
        self.st.borrow().cur_task.ok_or_else(|| {
            rt(format!("{what} waits for a result; call it inside rsb.task.spawn, rsb.poll, a timer or a handler"))
        })
    }

    fn exec_spec(
        &self,
        inst: InstanceId,
        program: ProgramRef,
        args: Vec<String>,
        opts: &Option<Table>,
        run: bool,
        shell: Option<String>,
    ) -> mlua::Result<ExecSpec> {
        let grant = self.grant(inst)?;
        let raw = shell.is_some();
        grant.permissions.check_exec_args(&program, (!raw).then_some(&args[..])).map_err(|n| self.deny(n))?;
        let timeout = match opt_get(opts, "timeout")? {
            Lv::Nil => run.then_some(iol::EXEC_DEFAULT_TIMEOUT),
            v => Some(duration_arg(&v, "timeout")?.min(iol::EXEC_MAX_TIMEOUT)),
        };
        let max_output = opt_usize(opts, "max_output", iol::EXEC_DEFAULT_OUTPUT, iol::EXEC_MAX_OUTPUT)?;
        let cwd = match opt_get(opts, "cwd")? {
            Lv::Nil => None,
            Lv::String(s) => Some(self.permitted_path(inst, &s.to_string_lossy())?),
            other => return Err(rt(format!("cwd must be a string, got {}", lua_type(&other)))),
        };
        Ok(ExecSpec { program, args, raw_tail: shell, cwd, timeout, max_output })
    }

    fn command_list(&self, cmd: &Lv) -> mlua::Result<(ProgramRef, Vec<String>)> {
        let Lv::Table(t) = cmd else {
            return Err(rt(format!(
                "the command must be a list like {{ \"git\", \"status\" }}, got {}",
                lua_type(cmd)
            )));
        };
        let mut words = Vec::new();
        for v in t.sequence_values::<Lv>() {
            if words.len() >= MAX_COMMAND_WORDS {
                return Err(rt(format!("a command can have at most {MAX_COMMAND_WORDS} words")));
            }
            match v? {
                Lv::String(s) => words.push(s.to_string_lossy()),
                other => return Err(rt(format!("command words must be strings, got {}", lua_type(&other)))),
            }
        }
        let Some(first) = words.first().cloned() else {
            return Err(rt("the command list is empty"));
        };
        let program = if is_abs_text(&first) {
            check_path_text(&first, false).map_err(rt)?;
            if !first.to_ascii_lowercase().ends_with(".exe") {
                return Err(rt(format!("'{first}' must be an .exe")));
            }
            ProgramRef::Path(PathBuf::from(first.replace('/', "\\")))
        } else {
            check_program_name(&first).map_err(rt)?;
            ProgramRef::Name(first)
        };
        Ok((program, words.split_off(1)))
    }

    fn send_wait(&self, inst: InstanceId, task: u64, request: IoRequest, kind: ResultKind) -> mlua::Result<()> {
        let id = self.next_io_id();
        let grant = self.grant(inst)?;
        {
            let mut st = self.st.borrow_mut();
            st.io.insert(id, (inst, IoWait::Task { task, kind }));
            if let Some(t) = st.tasks.get_mut(&task) {
                t.wait = Wait::Io(id);
            }
        }
        self.hub.send(HubRequest::Io { id, owner: inst, request, grant });
        Ok(())
    }

    fn service_arg(name: &str) -> mlua::Result<ServiceId> {
        ServiceId::from_name(name).ok_or_else(|| rt(format!("unknown service '{name}'")))
    }

    fn subscribe(&self, inst: InstanceId, svc: ServiceId, interval: Option<Duration>) -> mlua::Result<()> {
        let grant = self.grant(inst)?;
        grant.permissions.check_read(svc).map_err(|n| self.deny(n))?;
        let fresh = {
            let mut st = self.st.borrow_mut();
            let i = st.instances.get_mut(&inst).ok_or_else(|| rt("no such widget"))?;
            i.subs.insert(svc)
        };
        if fresh || interval.is_some() {
            self.hub.send(HubRequest::Subscribe { service: svc, owner: inst, interval });
        }
        Ok(())
    }
}

fn push_segment(out: &mut Vec<String>, seg: &str, original: &str, sh: &Shared) -> mlua::Result<()> {
    match seg {
        "" | "." => {}
        ".." => {
            if out.len() <= 1 {
                return Err(sh.kill(Stop::Permission(format!("file:{}", original.replace('\\', "/")))));
            }
            out.pop();
        }
        s => out.push(s.to_string()),
    }
    Ok(())
}

fn fold(out: &mut Vec<String>, base: &Path, sh: &Shared, original: &str) -> mlua::Result<()> {
    for c in base.components() {
        match c {
            Component::Prefix(p) => out.push(p.as_os_str().to_string_lossy().into_owned()),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => push_segment(out, "..", original, sh)?,
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
        }
    }
    Ok(())
}

fn collect(parts: &[String]) -> PathBuf {
    let mut s = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            s.push_str(p);
            s.push('\\');
        } else {
            if !s.ends_with('\\') {
                s.push('\\');
            }
            s.push_str(p);
        }
    }
    PathBuf::from(s)
}

/// Builds the `N` table for one VM.
pub(crate) fn install(lua: &Lua, sh: &Rc<Shared>, load_fn: &Rc<RefCell<Option<Function>>>) -> mlua::Result<Table> {
    let n = lua.create_table()?;

    macro_rules! native {
        ($name:literal, |$s:ident, $lua:pat_param, $arg:tt : $ty:ty| $body:expr) => {{
            let $s = sh.clone();
            n.set($name, lua.create_function(move |$lua, $arg: $ty| $body)?)?;
        }};
    }

    native!("is_killed", |s, _l, _a: ()| Ok(s.bud.killed.get()));
    native!("slice", |s, _l, _a: ()| s.slice());
    native!("report_error", |s, _l, (inst, msg): (i64, String)| {
        s.guard()?;
        match s.record_error(Some(InstanceId(inst as u32)), &msg) {
            Some(stop) => Err(s.kill(stop)),
            None => Ok(()),
        }
    });
    native!("update_loop", |s, _l, _inst: i64| Err::<(), Error>(s.kill(Stop::UpdateLoop)));
    native!("pending", |s, _l, inst: i64| {
        s.st.borrow_mut().pending_flush.insert(InstanceId(inst as u32));
        Ok(())
    });
    native!("dirty", |s, _l, inst: i64| {
        if let Some(i) = s.st.borrow_mut().instances.get_mut(&InstanceId(inst as u32)) {
            i.dirty = true;
        }
        Ok(())
    });
    native!("warn_untracked", |s, _l, inst: i64| {
        s.script_log(
            Some(InstanceId(inst as u32)),
            log::Level::Warn,
            "a signal was read in setup, so it will not update: read inside a function to keep it updated",
        );
        Ok(())
    });
    native!("wrap", |s, l, t: Table| {
        s.guard()?;
        l.create_any_userdata(ElementHandle(t))
    });
    native!("unwrap", |_s, _l, v: Lv| {
        Ok(match v {
            Lv::UserData(ud) => ud.borrow::<ElementHandle>().ok().map(|h| h.0.clone()),
            _ => None,
        })
    });
    native!("suggest", |_s, _l, (name, list): (String, Table)| {
        let names: Vec<String> = list.sequence_values::<String>().filter_map(Result::ok).collect();
        Ok(suggest(&name, &names))
    });
    native!("log", |s, _l, (level, msg): (String, String)| {
        s.guard()?;
        let lvl = match level.as_str() {
            "warn" => log::Level::Warn,
            "error" => log::Level::Error,
            _ => log::Level::Info,
        };
        s.script_log(s.current_instance(), lvl, &msg);
        Ok(())
    });

    // ---- handlers, tasks, timers
    native!("handler", |s, _l, f: Function| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let mut st = s.st.borrow_mut();
        let i = st.instances.get_mut(&inst).ok_or_else(|| rt("no such widget"))?;
        i.next_handler += 1;
        let id = i.next_handler;
        i.handlers.insert(id, f);
        Ok(id)
    });
    native!("unhandler", |s, _l, id: u32| {
        if let Some(inst) = s.current_instance()
            && let Some(i) = s.st.borrow_mut().instances.get_mut(&inst)
        {
            i.handlers.remove(&id);
        }
        Ok(())
    });
    native!("spawn", |s, l, args: MultiValue| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let mut it = args.into_iter();
        let Some(Lv::Function(f)) = it.next() else { return Err(rt("rsb.task.spawn needs a function")) };
        let rest = MultiValue::from_vec(it.collect());
        s.spawn_task(l, inst, f, rest)
    });
    native!("cancel", |s, _l, id: u64| {
        s.cancel_task(id);
        Ok(())
    });
    native!("sleep", |s, _l, d: Lv| {
        s.guard()?;
        let secs = duration_arg(&d, "rsb.task.sleep")?;
        let task = s.cur_task("rsb.task.sleep")?;
        let at = s.mono() + secs;
        if let Some(t) = s.st.borrow_mut().tasks.get_mut(&task) {
            t.wait = Wait::Sleep(at);
        }
        Ok(())
    });
    native!("timer", |s, _l, (kind, d, cb): (String, Lv, Function)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let lim = &s.env.limits;
        let mut period = duration_arg(&d, &format!("rsb.timer.{kind}"))?;
        let mut warn = false;
        if kind == "every" && period < lim.min_timer_interval {
            period = lim.min_timer_interval;
            warn = true;
        }
        let tk = if kind == "every" { TimerKind::Every { period, aligned: aligned(period) } } else { TimerKind::After };
        let first = match &tk {
            TimerKind::Every { aligned: true, .. } => next_aligned(&s, period),
            _ => period,
        };
        let id = {
            let mut st = s.st.borrow_mut();
            if st.timers.values().filter(|t| t.inst == inst).count() >= lim.max_timers_per_instance {
                return Err(rt(format!("too many timers (at most {} per widget)", lim.max_timers_per_instance)));
            }
            st.next_timer += 1;
            let id = st.next_timer;
            let next = s.mono() + first;
            st.timers.insert(id, TimerEnt { inst, kind: tk, next: Some(next), cb });
            if warn && let Some(i) = st.instances.get_mut(&inst) {
                warn = !std::mem::replace(&mut i.timer_warned, true);
            } else {
                warn = false;
            }
            id
        };
        if warn {
            s.script_log(Some(inst), log::Level::Warn, "timers faster than 100 ms are slowed to 100 ms");
        }
        Ok(id)
    });
    native!("timer_cancel", |s, _l, id: u64| {
        let me = s.current_instance();
        let mut st = s.st.borrow_mut();
        if st.timers.get(&id).is_some_and(|t| Some(t.inst) == me) {
            st.timers.remove(&id);
        }
        Ok(())
    });
    native!("clock_timer", |s, _l, (precision, zone, cb): (String, Lv, Function)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let precision = match precision.as_str() {
            "second" => Precision::Second,
            "minute" => Precision::Minute,
            "hour" => Precision::Hour,
            other => return Err(rt(format!("unknown clock precision '{other}'"))),
        };
        let zone = parse_zone(&zone)?;
        let wait = until_boundary(&s.env.clock.now(zone), precision);
        let mut st = s.st.borrow_mut();
        if st.timers.values().filter(|t| t.inst == inst).count() >= s.env.limits.max_timers_per_instance {
            return Err(rt("too many timers"));
        }
        st.next_timer += 1;
        let id = st.next_timer;
        let next = s.mono() + wait;
        st.timers.insert(id, TimerEnt { inst, kind: TimerKind::Clock { precision, zone }, next: Some(next), cb });
        Ok(id)
    });

    // ---- time
    native!("time_now", |s, l, zone: Lv| {
        s.guard()?;
        let z = parse_zone(&zone)?;
        time_table(l, &s.env.clock.now(z))
    });
    native!("time_format", |s, _l, (t, fmt): (Table, String)| {
        s.guard()?;
        Ok(format_time(&fmt, &time_from_table(&t)?))
    });
    native!("time_precision", |_s, _l, fmt: String| Ok(precision_of(&fmt).name()));

    // ---- services
    native!("touch", |s, _l, name: String| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let svc = Shared::service_arg(&name)?;
        s.subscribe(inst, svc, None)
    });
    native!("svc_interval", |s, _l, (name, d): (String, Lv)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let svc = Shared::service_arg(&name)?;
        let every = duration_arg(&d, "interval")?;
        s.subscribe(inst, svc, Some(every))
    });
    native!("svc_command", |s, _l, args: MultiValue| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let mut it = args.into_iter();
        let (Some(Lv::String(name)), Some(Lv::String(cmd))) = (it.next(), it.next()) else {
            return Err(rt("bad service command"));
        };
        let svc = Shared::service_arg(&name.to_string_lossy())?;
        let grant = s.grant(inst)?;
        grant.permissions.check_control(svc).map_err(|n| s.deny(n))?;
        let rest: Vec<Lv> = it.collect();
        let num = |i: usize, what: &str| -> mlua::Result<f32> {
            match rest.get(i) {
                Some(Lv::Integer(v)) => Ok(*v as f32),
                Some(Lv::Number(v)) => Ok(*v as f32),
                other => Err(rt(format!("{what} must be a number, got {}", other.map_or("nil", lua_type)))),
            }
        };
        let opt_monitor = || -> mlua::Result<Option<u32>> {
            match rest.first() {
                None | Some(Lv::Nil) => Ok(None),
                Some(Lv::Integer(v)) if *v >= 0 => Ok(Some(*v as u32)),
                Some(Lv::Number(v)) if *v >= 0.0 => Ok(Some(*v as u32)),
                other => Err(rt(format!("monitor must be a number, got {}", other.map_or("nil", lua_type)))),
            }
        };
        let command = match (svc, cmd.to_string_lossy().as_str()) {
            (ServiceId::Audio, "set_volume") => {
                ServiceCommand::Audio(AudioCommand::SetVolume(num(0, "percent")?.clamp(0.0, 100.0)))
            }
            (ServiceId::Audio, "change_volume") => ServiceCommand::Audio(AudioCommand::ChangeVolume(num(0, "delta")?)),
            (ServiceId::Audio, "set_muted") => match rest.first() {
                Some(Lv::Boolean(b)) => ServiceCommand::Audio(AudioCommand::SetMute(*b)),
                other => return Err(rt(format!("muted must be a boolean, got {}", other.map_or("nil", lua_type)))),
            },
            (ServiceId::Audio, "toggle_mute") => ServiceCommand::Audio(AudioCommand::ToggleMute),
            (ServiceId::Media, "play_pause") => ServiceCommand::Media(MediaCommand::PlayPause),
            (ServiceId::Media, "play") => ServiceCommand::Media(MediaCommand::Play),
            (ServiceId::Media, "pause") => ServiceCommand::Media(MediaCommand::Pause),
            (ServiceId::Media, "next") => ServiceCommand::Media(MediaCommand::Next),
            (ServiceId::Media, "previous") => ServiceCommand::Media(MediaCommand::Previous),
            (ServiceId::Workspaces, "focus") => match rest.first() {
                Some(Lv::String(name)) => ServiceCommand::Workspaces(WorkspaceCommand::Focus(name.to_string_lossy())),
                other => {
                    return Err(rt(format!(
                        "the workspace name must be a string, got {}",
                        other.map_or("nil", lua_type)
                    )));
                }
            },
            (ServiceId::Workspaces, "focus_next") => {
                ServiceCommand::Workspaces(WorkspaceCommand::FocusNext(opt_monitor()?))
            }
            (ServiceId::Workspaces, "focus_previous") => {
                ServiceCommand::Workspaces(WorkspaceCommand::FocusPrevious(opt_monitor()?))
            }
            (_, other) => return Err(rt(format!("{} has no command '{other}'", svc.name()))),
        };
        s.hub.send(HubRequest::Command { owner: inst, command });
        Ok(())
    });

    // ---- I/O
    native!("exec_run", |s, _l, (cmd, opts): (Lv, Option<Table>)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let task = s.cur_task("rsb.exec.run")?;
        let (program, args) = s.command_list(&cmd)?;
        let spec = s.exec_spec(inst, program, args, &opts, true, None)?;
        s.send_wait(inst, task, IoRequest::ExecRun(spec), ResultKind::Exec)
    });
    native!("exec_shell", |s, _l, (line, opts): (String, Option<Table>)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let task = s.cur_task("rsb.exec.shell")?;
        if line.chars().any(|c| c == '\0') {
            return Err(rt("the command line has a NUL character"));
        }
        let spec = s.exec_spec(inst, ProgramRef::Name("cmd".into()), Vec::new(), &opts, true, Some(line))?;
        s.send_wait(inst, task, IoRequest::ExecRun(spec), ResultKind::Exec)
    });
    native!(
        "exec_spawn",
        |s, _l, (cmd, opts, on_line, on_exit): (Lv, Option<Table>, Option<Function>, Option<Function>)| {
            s.guard()?;
            let inst = s.inst_or_err()?;
            let (program, args) = s.command_list(&cmd)?;
            let spec = s.exec_spec(inst, program, args, &opts, false, None)?;
            let id = s.next_io_id();
            let grant = s.grant(inst)?;
            s.st.borrow_mut().io.insert(id, (inst, IoWait::Spawn { on_line, on_exit }));
            s.hub.send(HubRequest::Io { id, owner: inst, request: IoRequest::ExecSpawn(spec), grant });
            Ok(id.0)
        }
    );
    native!("file_read", |s, _l, (path, opts): (String, Option<Table>)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let task = s.cur_task("rsb.file.read")?;
        let p = s.permitted_path(inst, &path)?;
        let max = opt_usize(&opts, "max_bytes", iol::FILE_DEFAULT_BYTES, iol::FILE_MAX_BYTES)?;
        s.send_wait(inst, task, IoRequest::ReadFile(FileRequest { path: p, max_bytes: max }), ResultKind::File)
    });
    native!("file_watch", |s, _l, (path, opts, cb): (String, Option<Table>, Function)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let p = s.permitted_path(inst, &path)?;
        let max = opt_usize(&opts, "max_bytes", iol::FILE_DEFAULT_BYTES, iol::FILE_MAX_BYTES)?;
        let id = s.next_io_id();
        let grant = s.grant(inst)?;
        s.st.borrow_mut().io.insert(id, (inst, IoWait::Watch { cb }));
        s.hub.send(HubRequest::Io {
            id,
            owner: inst,
            request: IoRequest::WatchFile(FileRequest { path: p, max_bytes: max }),
            grant,
        });
        Ok(id.0)
    });
    native!("io_cancel", |s, _l, id: u64| {
        let key = rsb_core::io::IoId(id);
        let me = s.current_instance();
        let mut st = s.st.borrow_mut();
        if st.io.get(&key).is_some_and(|(owner, _)| Some(*owner) == me) {
            st.io.remove(&key);
            drop(st);
            s.hub.send(HubRequest::Cancel { id: key });
        }
        Ok(())
    });
    native!("http", |s, l, (method, url, body, opts): (String, String, Lv, Option<Table>)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let task = s.cur_task(&format!("rsb.http.{}", method.to_ascii_lowercase()))?;
        let parsed = parse_url(&url).map_err(rt)?;
        let grant = s.grant(inst)?;
        grant.permissions.check_net(&parsed).map_err(|n| s.deny(n))?;
        let check = || s.check_limits(Some(l));
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Lv::Table(h) = opt_get(&opts, "headers")? {
            for pair in h.pairs::<String, String>() {
                if headers.len() >= iol::HTTP_MAX_HEADERS {
                    return Err(rt(format!("at most {} headers", iol::HTTP_MAX_HEADERS)));
                }
                headers.push(pair.map_err(|_| rt("headers must map names to strings"))?);
            }
        } else if !matches!(opt_get(&opts, "headers")?, Lv::Nil) {
            return Err(rt("headers must be a table"));
        }
        let body_bytes = match body {
            Lv::Nil => None,
            Lv::String(b) => Some(b.as_bytes().to_vec()),
            Lv::Table(t) => {
                let v = from_lua(&Lv::Table(t), &mut Walk::new(&check))?;
                if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                    headers.push(("Content-Type".into(), "application/json".into()));
                }
                Some(serde_json::to_vec(&v.to_json()).map_err(|e| rt(e.to_string()))?)
            }
            other => return Err(rt(format!("a request body is a string or a table, got {}", lua_type(&other)))),
        };
        check_headers(&headers).map_err(rt)?;
        let timeout = match opt_get(&opts, "timeout")? {
            Lv::Nil => iol::HTTP_DEFAULT_TIMEOUT,
            v => duration_arg(&v, "timeout")?.min(iol::HTTP_MAX_TIMEOUT),
        };
        let max_bytes = opt_usize(&opts, "max_bytes", iol::HTTP_DEFAULT_BYTES, iol::HTTP_MAX_BYTES)?;
        let cache_for = match opt_get(&opts, "cache")? {
            Lv::Nil => None,
            v => Some(duration_arg(&v, "cache")?),
        };
        let in_flight =
            s.st.borrow()
                .io
                .values()
                .filter(|(i, w)| *i == inst && matches!(w, IoWait::Task { kind: ResultKind::Http, .. }))
                .count();
        if in_flight >= iol::HTTP_MAX_IN_FLIGHT_PER_INSTANCE {
            let mut st = s.st.borrow_mut();
            if let Some(t) = st.tasks.get_mut(&task) {
                t.wait = Wait::Turn;
            }
            st.deferred.push((inst, Work::Resume { task, payload: Payload::Http(Err(rsb_core::io::IoError::Busy)) }));
            return Ok(());
        }
        let req = HttpRequest {
            method: if method == "POST" { HttpMethod::Post } else { HttpMethod::Get },
            url: url.clone(),
            headers,
            body: body_bytes,
            timeout,
            max_bytes,
            cache_for,
        };
        s.send_wait(inst, task, IoRequest::Http(req), ResultKind::Http)
    });
    native!("open_url", |s, _l, url: String| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let parsed = parse_url(&url).map_err(rt)?;
        if !parsed.https {
            return Err(rt("rsb.open_url only opens https:// addresses"));
        }
        rsb_core::permissions::open_url_text_ok(&url).map_err(rt)?;
        let grant = s.grant(inst)?;
        grant.permissions.check_net(&parsed).map_err(|n| s.deny(n))?;
        if !s.st.borrow().in_click {
            return Err(rt("rsb.open_url only works while a click is being handled"));
        }
        // One page every two seconds, however many widgets click at once.
        let now = s.mono();
        let recent = s.st.borrow().last_open.is_some_and(|t| now.saturating_duration_since(t) < OPEN_URL_GAP);
        if recent {
            s.script_log(Some(inst), log::Level::Warn, "rsb.open_url opens at most one address every 2 seconds");
            return Ok(());
        }
        s.st.borrow_mut().last_open = Some(now);
        let id = s.next_io_id();
        s.st.borrow_mut().io.insert(id, (inst, IoWait::Open));
        s.hub.send(HubRequest::Io { id, owner: inst, request: IoRequest::OpenUrl(url), grant });
        Ok(())
    });

    // ---- JSON
    native!("json_decode", |s, l, text: mlua::LuaString| {
        s.guard()?;
        let bytes = text.as_bytes();
        if bytes.len() > iol::JSON_MAX_BYTES {
            return Err(rt(format!("rsb.json.decode: text is larger than {} bytes", iol::JSON_MAX_BYTES)));
        }
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| rt(format!("rsb.json.decode: {e}")))?;
        // The parse is one call the interrupt cannot enter: look at the clock after each step.
        s.check_limits(Some(l))?;
        let value = Value::from_json(&v);
        if value.depth() > rsb_core::value::MAX_DEPTH {
            return Err(rt(format!("rsb.json.decode: nested over {} levels", rsb_core::value::MAX_DEPTH)));
        }
        s.check_limits(Some(l))?;
        let table = to_lua(l, &value, false)?;
        s.check_limits(Some(l))?;
        Ok(table)
    });
    native!("json_encode", |s, l, v: Lv| {
        s.guard()?;
        let check = || s.check_limits(Some(l));
        let value =
            from_lua(&v, &mut Walk::new(&check)).map_err(|e| rt(format!("rsb.json.encode: {}", error_text(&e))))?;
        serde_json::to_string(&value.to_json()).map_err(|e| rt(e.to_string()))
    });

    // ---- events
    native!("events_on", |s, _l, (name, cb): (String, Function)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let mut st = s.st.borrow_mut();
        if st.event_subs.values().filter(|e| e.inst == inst).count() >= 64 {
            return Err(rt("too many event subscriptions (at most 64 per widget)"));
        }
        st.next_sub += 1;
        let id = st.next_sub;
        st.event_subs.insert(id, EventSub { inst, name, cb });
        Ok(id)
    });
    native!("events_off", |s, _l, id: u64| {
        let me = s.current_instance();
        let mut st = s.st.borrow_mut();
        if st.event_subs.get(&id).is_some_and(|e| Some(e.inst) == me) {
            st.event_subs.remove(&id);
        }
        Ok(())
    });
    native!("events_emit", |s, l, (name, data): (String, Lv)| {
        s.guard()?;
        let inst = s.inst_or_err()?;
        let now = s.mono();
        let max = s.env.limits.max_events_per_sec;
        let allowed = {
            let mut st = s.st.borrow_mut();
            let Some(i) = st.instances.get_mut(&inst) else { return Ok(()) };
            let ok = i.event_win.allow(now, max);
            (ok, i.event_win.dropped)
        };
        if !allowed.0 {
            if allowed.1 == 1 {
                s.script_log(
                    Some(inst),
                    log::Level::Warn,
                    &format!("events are limited to {max} a second; extra events are dropped"),
                );
            }
            return Ok(());
        }
        let check = || s.check_limits(Some(l));
        let value = from_lua(&data, &mut Walk::new(&check))?;
        let size = value_bytes(&value);
        if size > EVENT_MAX_BYTES {
            return Err(rt(format!("rsb.events.emit: the data is larger than {} KiB", EVENT_MAX_BYTES >> 10)));
        }
        let mut st = s.st.borrow_mut();
        let refused = st.instances.get_mut(&inst).and_then(|i| {
            let ok = i.event_bytes_win.allow_cost(now, EVENT_BYTES_PER_SEC as u32, size.max(1) as u32);
            (!ok).then_some(i.event_bytes_win.dropped)
        });
        if let Some(dropped) = refused {
            drop(st);
            if dropped == 1 {
                s.script_log(
                    Some(inst),
                    log::Level::Warn,
                    &format!(
                        "events are limited to {} KiB of data a second; extra events are dropped",
                        EVENT_BYTES_PER_SEC >> 10
                    ),
                );
            }
            return Ok(());
        }
        // One copy of the data, shared by every subscriber's queued call.
        let value = Rc::new(value);
        let subs: Vec<(u64, InstanceId)> =
            st.event_subs.iter().filter(|(_, e)| e.name == name).map(|(k, e)| (*k, e.inst)).collect();
        for (sub, owner) in subs {
            st.deferred.push((owner, Work::Event { sub, data: Rc::clone(&value) }));
        }
        Ok(())
    });

    // ---- modules
    {
        let s = sh.clone();
        let load = load_fn.clone();
        n.set(
            "require",
            lua.create_function(move |l, (spec, from): (String, Option<String>)| {
                s.guard()?;
                require(&s, l, &load, &spec, from.as_deref().unwrap_or(""))
            })?,
        )?;
    }
    Ok(n)
}

fn require(
    sh: &Rc<Shared>,
    lua: &Lua,
    load: &Rc<RefCell<Option<Function>>>,
    spec: &str,
    from: &str,
) -> mlua::Result<Lv> {
    let inst = sh.inst_or_err()?;
    let from_rel = match from.strip_prefix("builtin/") {
        Some(rest) => format!("widgets/{rest}"),
        None => from.to_string(),
    };
    let root = {
        let st = sh.st.borrow();
        st.instances.get(&inst).map(|i| i.entry.clone()).unwrap_or_default()
    };
    let resolved = resolve_require(&root, &from_rel, spec).map_err(rt)?;
    {
        let st = sh.st.borrow();
        let i = st.instances.get(&inst).ok_or_else(|| rt("no such widget"))?;
        if let Some(v) = i.modules.get(&resolved) {
            return Ok(v.clone());
        }
        if i.loading.contains(&resolved) {
            return Err(rt(format!("require(\"{spec}\") is a cycle: {} needs itself", resolved)));
        }
    }
    let candidates = [format!("{resolved}.luau"), format!("{resolved}/init.luau")];
    // A built-in widget only ever loads built-in modules: a file of the same name in the user's widgets folder
    // would otherwise run inside it, with the permissions the built-in holds.
    let builtin = {
        let mut st = sh.st.borrow_mut();
        match st.instances.get_mut(&inst) {
            Some(i) => {
                if !i.builtin {
                    for c in &candidates {
                        i.files.insert(c.clone());
                    }
                }
                i.builtin
            }
            None => false,
        }
    };
    let mut found: Option<(String, String)> = None;
    for c in &candidates {
        if !builtin && let Some(src) = sh.user_source(c).map_err(rt)? {
            found = Some((c.clone(), src));
            break;
        }
        if let Some(src) = sh.env.sources.builtin(c) {
            let chunk = format!("builtin/{}", c.strip_prefix("widgets/").unwrap_or(c));
            found = Some((chunk, src.into_owned()));
            break;
        }
    }
    let Some((chunk, src)) = found else {
        return Err(rt(format!("module '{spec}' not found")));
    };
    let f = lua.load(src.as_str()).set_name(format!("={chunk}")).into_function()?;
    let loader = load.borrow().clone().ok_or_else(|| rt("the runtime is not ready"))?;
    if let Some(i) = sh.st.borrow_mut().instances.get_mut(&inst) {
        i.loading.push(resolved.clone());
    }
    let r = loader.call::<Lv>(f);
    if let Some(i) = sh.st.borrow_mut().instances.get_mut(&inst) {
        i.loading.retain(|x| x != &resolved);
    }
    let v = match r? {
        Lv::Nil => Lv::Boolean(true),
        other => other,
    };
    if let Some(i) = sh.st.borrow_mut().instances.get_mut(&inst) {
        i.modules.insert(resolved, v.clone());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_use_edit_distance_two() {
        let names: Vec<String> = ["color", "background", "font_size"].iter().map(|s| s.to_string()).collect();
        assert_eq!(suggest("colr", &names).as_deref(), Some("color"));
        assert_eq!(suggest("font_sise", &names).as_deref(), Some("font_size"));
        assert_eq!(suggest("zzzzzz", &names), None);
        assert_eq!(suggest("x", &names), None);
    }

    #[test]
    fn timers_align_to_whole_units() {
        assert!(aligned(Duration::from_secs(60)));
        assert!(aligned(Duration::from_secs(1)));
        assert!(aligned(Duration::from_secs(300)));
        assert!(!aligned(Duration::from_secs(7)));
        assert!(!aligned(Duration::from_millis(1500)));
        assert!(!aligned(Duration::from_secs(7200)));
    }
}
