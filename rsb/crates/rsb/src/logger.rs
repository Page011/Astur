//! The log: a size-capped rotating file behind the `log` facade, plus the
//! panic hook.
//!
//! `rsb.log` is the live file; when the next line would pass 5 MiB it becomes
//! `rsb.1.log`, then `rsb.2.log`, and the oldest beyond three files is
//! deleted. Lines look like
//! `2026-11-24T20:15:02.123+10:00 INFO rsb_app::run: started` (local time with
//! its UTC offset, as the bar shows it). They carry counts,
//! sizes and timings, never window titles, media names, network names or
//! paths outside the config folder.
//!
//! A real Rust panic is fatal: the hook writes the message and location and
//! flushes, then aborts, so no thread dies quietly and leaves a frozen bar.
//! (Script errors do not come through here: they are Luau errors.)

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

const STEM: &str = "rsb";
/// A runaway message must not eat a whole file.
const MAX_LINE: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rotation {
    /// Files kept, the live one included (at least 1).
    pub max_files: u32,
    /// Size at which the live file rotates (at least 1 KiB).
    pub max_bytes: u64,
}

impl Default for Rotation {
    fn default() -> Self {
        Rotation { max_files: 3, max_bytes: 5 * 1024 * 1024 }
    }
}

/// `rsb.log`, `rsb.1.log`, `rsb.2.log` for index 0, 1, 2.
pub fn file_path(dir: &Path, index: u32) -> PathBuf {
    if index == 0 { dir.join(format!("{STEM}.log")) } else { dir.join(format!("{STEM}.{index}.log")) }
}

pub struct RotatingFile {
    dir: PathBuf,
    policy: Rotation,
    file: File,
    len: u64,
    /// After a failed rotation, the size at which to try again, so a locked
    /// file is not retried on every line.
    retry_at: Option<u64>,
}

fn open_live(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl RotatingFile {
    pub fn open(dir: &Path, policy: Rotation) -> io::Result<RotatingFile> {
        let policy = Rotation { max_files: policy.max_files.max(1), max_bytes: policy.max_bytes.max(1024) };
        fs::create_dir_all(dir)?;
        let file = open_live(&file_path(dir, 0))?;
        let len = file.metadata()?.len();
        let mut log = RotatingFile { dir: dir.to_path_buf(), policy, file, len, retry_at: None };
        if log.len > policy.max_bytes && log.rotate().is_err() {
            log.retry_at = Some(log.len + policy.max_bytes);
        }
        Ok(log)
    }

    /// Writes one line and flushes, rotating first when it would cross the
    /// limit. A rotation failure is returned once; the line is written anyway.
    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let mut data = Vec::with_capacity(line.len() + 1);
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        let mut rotation_error = None;
        let crossing = self.len > 0 && self.len + data.len() as u64 > self.policy.max_bytes;
        if crossing && self.retry_at.is_none_or(|at| self.len >= at) {
            match self.rotate() {
                Ok(()) => self.retry_at = None,
                Err(e) => {
                    self.retry_at = Some(self.len + self.policy.max_bytes);
                    rotation_error = Some(e);
                }
            }
        }
        self.file.write_all(&data)?;
        self.file.flush()?;
        self.len += data.len() as u64;
        rotation_error.map_or(Ok(()), Err)
    }

    fn rotate(&mut self) -> io::Result<()> {
        let live = file_path(&self.dir, 0);
        if self.policy.max_files == 1 {
            self.file = File::create(&live)?;
            self.file = open_live(&live)?;
            self.len = 0;
            return Ok(());
        }
        // Oldest slot first, so nothing is overwritten before it has moved.
        for index in (0..self.policy.max_files - 1).rev() {
            let from = file_path(&self.dir, index);
            if from.exists() {
                fs::rename(&from, file_path(&self.dir, index + 1))?;
            }
        }
        match open_live(&live) {
            Ok(file) => {
                self.file = file;
                self.len = 0;
                Ok(())
            }
            Err(e) => {
                let _ = fs::rename(file_path(&self.dir, 1), &live);
                Err(e)
            }
        }
    }
}

struct FileLogger {
    file: Mutex<RotatingFile>,
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// `2026-11-24T10:15:02.123Z` for a time since the Unix epoch.
pub fn timestamp(since_epoch: Duration) -> String {
    timestamp_at(since_epoch, 0)
}

/// The same moment as `timestamp`, written in a zone `offset_min` minutes
/// from UTC: `2026-11-24T20:15:02.123+10:00`, or with `Z` at offset 0.
pub fn timestamp_at(since_epoch: Duration, offset_min: i32) -> String {
    let secs = since_epoch.as_secs() as i64 + i64::from(offset_min) * 60;
    let (year, month, day) = rsb_core::time::civil_from_days(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    let zone = if offset_min == 0 {
        "Z".to_string()
    } else {
        let sign = if offset_min < 0 { '-' } else { '+' };
        format!("{sign}{:02}:{:02}", offset_min.abs() / 60, offset_min.abs() % 60)
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}{zone}",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60,
        since_epoch.subsec_millis()
    )
}

fn now_stamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let offset = rsb_core::time::Clock::now(&rsb_services::clock::SystemClock, rsb_core::time::TimeZone::Local);
    timestamp_at(now, offset.utc_offset_min)
}

/// One finished line: control characters (a forged newline included) become
/// spaces and the message is cut to `MAX_LINE` on a char boundary.
pub fn format_line(stamp: &str, level: Level, target: &str, message: &str) -> String {
    let mut text: String = message
        .chars()
        .map(|c| if c.is_control() || rsb_core::diag::is_invisible_control(c) { ' ' } else { c })
        .collect();
    if text.len() > MAX_LINE {
        let mut cut = MAX_LINE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("...");
    }
    format!("{stamp} {level} {target}: {text}")
}

impl FileLogger {
    fn write(&self, line: &str) {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = file.write_line(line) {
            eprintln!("rsb: log write: {e}");
        }
    }

    /// For the panic hook: never wait for a lock the panicking thread may hold.
    fn write_now(&self, line: &str) {
        for _ in 0..50 {
            match self.file.try_lock() {
                Ok(mut f) => {
                    let _ = f.write_line(line);
                    return;
                }
                Err(std::sync::TryLockError::Poisoned(p)) => {
                    let _ = p.into_inner().write_line(line);
                    return;
                }
                Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
        eprintln!("{line}");
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.write(&format_line(&now_stamp(), record.level(), record.target(), &record.args().to_string()));
        }
    }

    fn flush(&self) {}
}

/// Opens the log folder, installs the logger and the panic hook. Call once,
/// before anything else logs. An error is returned for the caller to print;
/// the bar still runs (unlogged) rather than refusing to start.
pub fn init(dir: &Path) -> Result<PathBuf, String> {
    let file = RotatingFile::open(dir, Rotation::default()).map_err(|e| format!("cannot open the log folder: {e}"))?;
    if LOGGER.set(FileLogger { file: Mutex::new(file) }).is_err() {
        return Err("the logger is already installed".into());
    }
    let logger = LOGGER.get().expect("the logger was just set");
    log::set_logger(logger).map_err(|e| format!("cannot install the logger: {e}"))?;
    log::set_max_level(LevelFilter::Info);
    std::panic::set_hook(Box::new(|info| {
        let place =
            info.location().map_or_else(|| "an unknown place".to_string(), |l| format!("{}:{}", l.file(), l.line()));
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        let thread = std::thread::current();
        let line = format_line(
            &now_stamp(),
            Level::Error,
            "panic",
            &format!("thread '{}' panicked at {place}: {message}", thread.name().unwrap_or("unnamed")),
        );
        match LOGGER.get() {
            Some(l) => l.write_now(&line),
            None => eprintln!("{line}"),
        }
        // An aborted process cannot unregister its appbars, and the shell would keep the space.
        rsb_win::remove_all_appbars();
        std::process::abort();
    }));
    Ok(file_path(dir, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn line(n: u32) -> String {
        format!("{n:05} {}", "x".repeat(93))
    }

    #[test]
    fn ten_kib_into_one_kib_files_keeps_three_files_in_order() {
        let dir = TempDir::new("log");
        let mut log = RotatingFile::open(dir.path(), Rotation { max_files: 3, max_bytes: 1024 }).unwrap();
        for n in 0..104 {
            log.write_line(&line(n)).unwrap();
        }
        assert_eq!(names(dir.path()), ["rsb.1.log", "rsb.2.log", "rsb.log"]);
        let mut all = String::new();
        for f in ["rsb.2.log", "rsb.1.log", "rsb.log"] {
            assert!(fs::metadata(dir.path().join(f)).unwrap().len() <= 1024, "{f}");
            all.push_str(&fs::read_to_string(dir.path().join(f)).unwrap());
        }
        let numbers: Vec<u32> = all.lines().map(|l| l[..5].parse().unwrap()).collect();
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "{numbers:?}");
        assert_eq!(numbers.last(), Some(&103));
        assert!(!all.contains(&line(0)), "the oldest line is gone");
    }

    #[test]
    fn one_file_starts_over_in_place_and_reopen_appends() {
        let dir = TempDir::new("log");
        let mut log = RotatingFile::open(dir.path(), Rotation { max_files: 1, max_bytes: 1024 }).unwrap();
        for n in 0..50 {
            log.write_line(&line(n)).unwrap();
        }
        assert_eq!(names(dir.path()), ["rsb.log"]);
        drop(log);
        let mut log = RotatingFile::open(dir.path(), Rotation { max_files: 3, max_bytes: 1 << 20 }).unwrap();
        log.write_line("again").unwrap();
        assert!(fs::read_to_string(dir.path().join("rsb.log")).unwrap().ends_with("again\n"));
    }

    #[test]
    fn timestamps_and_lines_are_exact() {
        assert_eq!(timestamp(Duration::ZERO), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(Duration::new(1_709_251_199, 999_000_000)), "2024-02-29T23:59:59.999Z");
        assert_eq!(timestamp_at(Duration::new(1_709_251_199, 5_000_000), 600), "2024-03-01T09:59:59.005+10:00");
        assert_eq!(timestamp_at(Duration::new(1_709_251_199, 0), -330), "2024-02-29T18:29:59.000-05:30");
        assert_eq!(timestamp_at(Duration::ZERO, 0), timestamp(Duration::ZERO));
        // The line that is written carries a zone: Z or a numeric offset.
        let stamp = now_stamp();
        assert!(stamp.ends_with('Z') || stamp[stamp.len() - 6..].starts_with(['+', '-']), "{stamp}");
        assert_eq!(format_line("T", Level::Warn, "x", "one\ntwo\r\nthree\u{1b}"), "T WARN x: one two  three ");
        assert_eq!(format_line("T", Level::Info, "x", "a\u{2028}b\u{202e}c"), "T INFO x: a b c");
        let long = format_line("T", Level::Info, "x", &"\u{e9}".repeat(5000));
        assert!(long.len() < MAX_LINE + 20 && long.ends_with("..."));
    }
}
