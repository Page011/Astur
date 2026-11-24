//! The measurement file behind `--stats-out`: one JSON object per line,
//! `{"t_ms":..,"wall_ms":..,"session":"rdp"|"console","event":..,...}`.
//! Values never include window titles, media names, network names or paths.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};

/// "rdp" in a Remote Desktop session, else "console". Every timing taken in
/// an rdp session says so.
pub fn session_name() -> &'static str {
    // SAFETY: a plain query with no pointers.
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 { "rdp" } else { "console" }
}

pub fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub struct Stats {
    file: Mutex<Option<File>>,
    start: Instant,
    session: &'static str,
    /// `--stats-frames`: one line per presented frame.
    pub frames: bool,
}

impl Stats {
    /// `path` None = stats off (every call is a cheap no-op).
    pub fn open(path: Option<&Path>, frames: bool, start: Instant) -> Result<Stats, String> {
        let file = match path {
            Some(p) => Some(File::create(p).map_err(|e| format!("cannot open the stats file: {e}"))?),
            None => None,
        };
        Ok(Stats { file: Mutex::new(file), start, session: session_name(), frames })
    }

    pub fn off() -> Stats {
        Stats { file: Mutex::new(None), start: Instant::now(), session: session_name(), frames: false }
    }

    pub fn enabled(&self) -> bool {
        self.file.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub fn t_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// One line. `fields` must be a JSON object (or null for none).
    pub fn emit(&self, event: &str, fields: Value) {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let Some(file) = guard.as_mut() else { return };
        let mut line = Map::new();
        line.insert("t_ms".into(), json!(self.start.elapsed().as_millis() as u64));
        line.insert("wall_ms".into(), json!(wall_ms()));
        line.insert("session".into(), json!(self.session));
        line.insert("event".into(), json!(event));
        if let Value::Object(extra) = fields {
            line.extend(extra);
        }
        let _ = writeln!(file, "{}", Value::Object(line));
        let _ = file.flush();
    }
}

fn filetime_u64(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Milliseconds from this process's creation to now.
pub fn since_process_start_ms() -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the pseudo handle is always valid and every out pointer is live.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    // SAFETY: no arguments; returns the time by value.
    let now = filetime_u64(unsafe { GetSystemTimeAsFileTime() });
    Some(now.saturating_sub(filetime_u64(created)) / 10_000)
}

/// How coarse `cpu_ms` is: Windows adds to a process's times in scheduler
/// ticks, so a short run can read several milliseconds off in either direction.
/// Judge CPU use over a minute or more, not from one short run.
pub const CPU_TIME_RESOLUTION_MS: f64 = 15.6;

/// Process CPU time (kernel + user) in milliseconds, in steps of `CPU_TIME_RESOLUTION_MS`.
pub fn cpu_ms() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the pseudo handle is always valid and every out pointer is live.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.is_err() {
        return 0.0;
    }
    (filetime_u64(kernel) + filetime_u64(user)) as f64 / 10_000.0
}

/// (private bytes, working set) in MB.
pub fn memory_mb() -> (f64, f64) {
    let mut counters =
        PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    let me: HANDLE;
    // SAFETY: the pseudo handle is always valid.
    unsafe {
        me = GetCurrentProcess();
    }
    // SAFETY: `counters` is a live PROCESS_MEMORY_COUNTERS_EX, which starts with a PROCESS_MEMORY_COUNTERS.
    let ok = unsafe {
        GetProcessMemoryInfo(
            me,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    if ok.is_err() {
        return (0.0, 0.0);
    }
    let mb = |bytes: usize| (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0;
    (mb(counters.PrivateUsage), mb(counters.WorkingSetSize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn lines_are_json_with_the_four_fixed_keys_and_off_is_silent() {
        let dir = TempDir::new("stats");
        let path = dir.path().join("s.jsonl");
        let stats = Stats::open(Some(&path), false, Instant::now()).unwrap();
        assert!(stats.enabled());
        stats.emit("start", Value::Null);
        stats.emit("first_frame", json!({"ms": 42}));
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        for l in &lines {
            for key in ["t_ms", "wall_ms", "session", "event"] {
                assert!(l.get(key).is_some(), "{key} in {l}");
            }
        }
        assert_eq!(lines[1]["ms"], 42);
        assert!(["rdp", "console"].contains(&lines[0]["session"].as_str().unwrap()));
        let off = Stats::off();
        off.emit("x", Value::Null);
        assert!(!off.enabled());
    }

    #[test]
    fn the_process_numbers_are_plausible() {
        let (private, working) = memory_mb();
        assert!(private > 1.0 && working > 1.0, "{private} {working}");
        assert!(cpu_ms() >= 0.0);
        assert!(since_process_start_ms().is_some_and(|ms| ms < 3_600_000));
    }
}
