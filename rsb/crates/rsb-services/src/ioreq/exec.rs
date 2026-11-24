//! `rsb.exec.run` / `spawn`: programs started suspended, put in a job object
//! before they run (so killing a widget kills the whole tree), with only
//! their own output pipes inherited.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsb_core::element::InstanceId;
use rsb_core::io::{ExecResult, ExecSpec, IoError, IoEventKind, IoGrant, IoId, ProgramRef, limits};
use rsb_core::permissions::program_key;
use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForMultipleObjects,
};
use windows::core::{PCWSTR, PWSTR};

use super::send;
use crate::Sink;
use crate::win::{Owned, manual_event, signal, wide};

const RUNNING: u8 = 0;
/// Killed by `Cancel { id }`: the requester is told.
const CANCELLED: u8 = 1;
/// Killed by `CancelOwner` or shutdown: nobody is listening, no event.
const SILENT: u8 = 2;

/// After the program exits its output readers get this long to reach end of
/// file; a descendant still holding the pipe is then killed with the job.
const READER_GRACE: Duration = Duration::from_secs(2);

struct Proc {
    owner: InstanceId,
    cancel: Arc<Owned>,
    mode: Arc<AtomicU8>,
}

#[derive(Default)]
struct State {
    procs: HashMap<IoId, Proc>,
}

pub(crate) struct ExecManager {
    sink: Arc<Sink>,
    state: Arc<Mutex<State>>,
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

impl ExecManager {
    pub(crate) fn new(sink: Arc<Sink>) -> ExecManager {
        ExecManager { sink, state: Arc::default() }
    }

    pub(crate) fn start(&self, id: IoId, owner: InstanceId, spec: ExecSpec, grant: Arc<IoGrant>, stream: bool) {
        let cancel = match manual_event() {
            Ok(e) => Arc::new(e),
            Err(e) => {
                send(&self.sink, id, owner, IoEventKind::ExecDone(Err(IoError::Spawn(e))));
                return;
            }
        };
        let mode = Arc::new(AtomicU8::new(RUNNING));
        {
            let mut st = lock(&self.state);
            let mine = st.procs.values().filter(|p| p.owner == owner).count();
            if mine >= limits::EXEC_MAX_PROCESSES_PER_INSTANCE || st.procs.len() >= limits::EXEC_MAX_PROCESSES_TOTAL {
                drop(st);
                send(&self.sink, id, owner, IoEventKind::ExecDone(Err(IoError::Busy)));
                return;
            }
            st.procs.insert(id, Proc { owner, cancel: cancel.clone(), mode: mode.clone() });
        }
        let sink = self.sink.clone();
        let state = self.state.clone();
        let spawned = std::thread::Builder::new().name("rsb-exec".into()).spawn(move || {
            let result = run_process(&sink, id, owner, &spec, &grant, stream, &cancel, &mode);
            lock(&state).procs.remove(&id);
            match mode.load(Ordering::Acquire) {
                SILENT => {}
                CANCELLED => send(&sink, id, owner, IoEventKind::ExecDone(Err(IoError::Cancelled))),
                _ => send(&sink, id, owner, IoEventKind::ExecDone(result)),
            }
        });
        if spawned.is_err() {
            lock(&self.state).procs.remove(&id);
            send(&self.sink, id, owner, IoEventKind::ExecDone(Err(IoError::Busy)));
        }
    }

    pub(crate) fn cancel(&self, id: IoId) {
        if let Some(p) = lock(&self.state).procs.get(&id) {
            let _ = p.mode.compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
            signal(&p.cancel);
        }
    }

    pub(crate) fn cancel_owner(&self, owner: InstanceId) {
        for p in lock(&self.state).procs.values().filter(|p| p.owner == owner) {
            p.mode.store(SILENT, Ordering::Release);
            signal(&p.cancel);
        }
    }

    pub(crate) fn shutdown(&self) {
        for p in lock(&self.state).procs.values() {
            p.mode.store(SILENT, Ordering::Release);
            signal(&p.cancel);
        }
        super::wait_until(Duration::from_millis(800), || lock(&self.state).procs.is_empty());
    }
}

// ------------------------------------------------------------ command line

/// Quotes one argument for a Windows command line so `split_command_line`
/// (and the C runtime) read it back unchanged.
pub(crate) fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// The program on PATH (absolute entries only, `.exe` only) or the absolute
/// path asked for. Never the current folder or the app folder.
pub(crate) fn resolve_program(program: &ProgramRef) -> Result<PathBuf, IoError> {
    match program {
        ProgramRef::Name(name) => {
            let file = format!("{}.exe", program_key(name));
            let path = std::env::var_os("PATH").unwrap_or_default();
            for dir in std::env::split_paths(&path) {
                if dir.is_absolute() {
                    let candidate = dir.join(&file);
                    if candidate.is_file() {
                        return Ok(candidate);
                    }
                }
            }
            Err(IoError::Spawn("the program was not found on PATH".into()))
        }
        ProgramRef::Path(p) => {
            let is_exe = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
            if !p.is_absolute() || !is_exe {
                return Err(IoError::BadRequest("a program path must be an absolute path to an .exe".into()));
            }
            if p.is_file() { Ok(p.clone()) } else { Err(IoError::Spawn("the program was not found".into())) }
        }
    }
}

fn command_line(exe: &Path, spec: &ExecSpec) -> String {
    let mut line = format!("\"{}\"", exe.to_string_lossy());
    match &spec.raw_tail {
        Some(tail) => {
            line.push_str(" /d /c ");
            line.push_str(tail);
        }
        None => {
            for a in &spec.args {
                line.push(' ');
                line.push_str(&quote_arg(a));
            }
        }
    }
    line
}

// -------------------------------------------------------------- the process

struct Pipes {
    read: Owned,
    write: Owned,
}

fn make_pipe() -> Result<Pipes, IoError> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let (mut r, mut w) = (HANDLE::default(), HANDLE::default());
    // SAFETY: both out pointers are live; the attributes are fully initialised.
    unsafe { CreatePipe(&mut r, &mut w, Some(&sa), 0) }.map_err(|e| IoError::Spawn(format!("pipe: {e}")))?;
    let (read, write) = (Owned(r), Owned(w));
    // Our end is not inherited by anyone.
    // SAFETY: `read` is a live handle.
    unsafe { SetHandleInformation(read.get(), HANDLE_FLAG_INHERIT.0, Default::default()) }
        .map_err(|e| IoError::Spawn(format!("pipe: {e}")))?;
    Ok(Pipes { read, write })
}

fn open_nul() -> Result<Owned, IoError> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let name = wide("NUL");
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&sa),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map(Owned)
    .map_err(|e| IoError::Spawn(format!("stdin: {e}")))
}

fn make_job() -> Result<Owned, IoError> {
    // SAFETY: no pointers are passed; the job handle is owned by the result.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
        .map(Owned)
        .map_err(|e| IoError::Spawn(format!("job: {e}")))?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `info` is a valid JOBOBJECT_EXTENDED_LIMIT_INFORMATION and the size passed is its size.
    unsafe {
        SetInformationJobObject(
            job.get(),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    }
    .map_err(|e| IoError::Spawn(format!("job: {e}")))?;
    Ok(job)
}

struct Started {
    process: Owned,
    job: Owned,
    out: Owned,
    err: Owned,
}

fn create(exe: &Path, line: &str, cwd: &Path) -> Result<Started, IoError> {
    let out = make_pipe()?;
    let err = make_pipe()?;
    let nul = open_nul()?;
    let job = make_job()?;

    // Only these three handles are inherited by the child.
    let inherited = [nul.get(), out.write.get(), err.write.get()];
    let mut size = 0usize;
    // SAFETY: a size query; it fails with the needed length, which is the expected result.
    let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut size) };
    let mut attr_buf = vec![0usize; size.div_ceil(size_of::<usize>()).max(1)];
    let attrs = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr().cast());
    // SAFETY: `attr_buf` is at least `size` bytes, aligned for a pointer.
    unsafe { InitializeProcThreadAttributeList(Some(attrs), 1, None, &mut size) }
        .map_err(|e| IoError::Spawn(format!("attributes: {e}")))?;
    // SAFETY: `inherited` outlives CreateProcessW below; the list holds the pointer, not a copy.
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(inherited.as_ptr().cast()),
            size_of_val(&inherited),
            None,
            None,
        )
    };
    if let Err(e) = updated {
        // SAFETY: the list was initialised above.
        unsafe { DeleteProcThreadAttributeList(attrs) };
        return Err(IoError::Spawn(format!("attributes: {e}")));
    }

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    startup.StartupInfo.hStdInput = nul.get();
    startup.StartupInfo.hStdOutput = out.write.get();
    startup.StartupInfo.hStdError = err.write.get();
    startup.StartupInfo.wShowWindow = 0; // SW_HIDE
    startup.lpAttributeList = attrs;

    let app = wide(&exe.to_string_lossy());
    let mut cmd = wide(line);
    let dir = wide(&cwd.to_string_lossy());
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every buffer is NUL-terminated and outlives the call; `startup` is a valid STARTUPINFOEXW
    // (EXTENDED_STARTUPINFO_PRESENT) whose attribute list is initialised.
    let created = unsafe {
        CreateProcessW(
            PCWSTR(app.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR(dir.as_ptr()),
            (&raw const startup.StartupInfo).cast(),
            &mut info,
        )
    };
    // SAFETY: the list was initialised above and is no longer needed.
    unsafe { DeleteProcThreadAttributeList(attrs) };
    created.map_err(|e| IoError::Spawn(spawn_reason(&e)))?;
    let process = Owned(info.hProcess);
    let thread = Owned(info.hThread);

    // Into the job before the first instruction runs, so nothing it starts can escape.
    // SAFETY: both handles are live.
    if let Err(e) = unsafe { AssignProcessToJobObject(job.get(), process.get()) } {
        // SAFETY: terminates the suspended child we just made.
        let _ = unsafe { windows::Win32::System::Threading::TerminateProcess(process.get(), 1) };
        return Err(IoError::Spawn(format!("job: {e}")));
    }
    // SAFETY: `thread` is the suspended main thread of the new process.
    unsafe { ResumeThread(thread.get()) };
    // Our copies of the child's ends close here (the Pipes drop), so the
    // readers see end of file when the child side is gone.
    Ok(Started { process, job, out: out.read, err: err.read })
}

/// A readable reason that never names a path.
fn spawn_reason(e: &windows::core::Error) -> String {
    let code = e.code().0 & 0xFFFF;
    match code {
        2 | 3 => "the program or its folder was not found".into(),
        5 => "access to the program was denied".into(),
        193 => "the file is not a valid program".into(),
        267 => "the working folder is not valid".into(),
        _ => format!("Windows error {code}"),
    }
}

// ------------------------------------------------------------------ readers

struct Limiter {
    window: Instant,
    count: u32,
    dropped: u64,
}

struct Stream {
    sink: Arc<Sink>,
    id: IoId,
    owner: InstanceId,
    mode: Arc<AtomicU8>,
    limiter: Arc<Mutex<Limiter>>,
}

impl Stream {
    fn line(&self, stderr: bool, bytes: &[u8], cut: bool) {
        if self.mode.load(Ordering::Acquire) != RUNNING {
            return;
        }
        {
            let mut l = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
            if l.window.elapsed() >= Duration::from_secs(1) {
                l.window = Instant::now();
                l.count = 0;
            }
            if l.count >= limits::EXEC_MAX_LINES_PER_SEC {
                l.dropped += 1;
                return;
            }
            l.count += 1;
        }
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        if cut {
            text.push_str(" [cut]");
        }
        send(&self.sink, self.id, self.owner, IoEventKind::ExecLine { stderr, line: text });
    }
}

enum Sinkmode {
    Collect { max: usize },
    Lines(Stream),
}

struct Collected {
    bytes: Vec<u8>,
    truncated: bool,
}

/// Reads until the pipe closes. `Collect` keeps up to `max` bytes and drains
/// the rest; `Lines` emits whole lines.
fn read_pipe(pipe: Owned, stderr: bool, mode: Sinkmode) -> Collected {
    let mut buf = vec![0u8; 8192];
    let mut collected = Collected { bytes: Vec::new(), truncated: false };
    let mut pending: Vec<u8> = Vec::new();
    let mut discarding = false;
    loop {
        let mut n = 0u32;
        // SAFETY: `buf` is a writable slice and `n` a live out pointer; the handle is the pipe's read end.
        let r = unsafe { ReadFile(pipe.get(), Some(&mut buf), Some(&mut n), None) };
        if r.is_err() || n == 0 {
            break;
        }
        let chunk = &buf[..n as usize];
        match &mode {
            Sinkmode::Collect { max } => {
                let room = max.saturating_sub(collected.bytes.len());
                if chunk.len() > room {
                    collected.truncated = true;
                }
                collected.bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Sinkmode::Lines(stream) => {
                for &b in chunk {
                    if b == b'\n' {
                        if !discarding {
                            if pending.last() == Some(&b'\r') {
                                pending.pop();
                            }
                            stream.line(stderr, &pending, false);
                        }
                        pending.clear();
                        discarding = false;
                    } else if !discarding {
                        pending.push(b);
                        if pending.len() >= limits::EXEC_MAX_LINE {
                            stream.line(stderr, &pending, true);
                            pending.clear();
                            // The rest of an over-long line is dropped.
                            discarding = true;
                        }
                    }
                }
            }
        }
    }
    if let Sinkmode::Lines(stream) = &mode
        && !pending.is_empty()
        && !discarding
    {
        if pending.last() == Some(&b'\r') {
            pending.pop();
        }
        stream.line(stderr, &pending, false);
    }
    collected
}

// ----------------------------------------------------------------- the run

#[allow(clippy::too_many_arguments)]
fn run_process(
    sink: &Arc<Sink>,
    id: IoId,
    owner: InstanceId,
    spec: &ExecSpec,
    grant: &IoGrant,
    stream: bool,
    cancel: &Owned,
    mode: &Arc<AtomicU8>,
) -> Result<ExecResult, IoError> {
    // The runtime checked this already; checking again costs nothing.
    let raw = spec.raw_tail.is_some();
    if let Err(need) = grant.permissions.check_exec_args(&spec.program, (!raw).then_some(&spec.args[..])) {
        return Err(IoError::NotPermitted(need.to_string()));
    }
    let exe = resolve_program(&spec.program)?;
    let line = command_line(&exe, spec);
    // Windows looks for DLLs, and many programs look for their config and
    // modules, in the working folder. A widget's own folder is the one place
    // its author (or a package) controls, so the default is the program's own
    // folder, which Windows searches first anyway. A widget that wants its own
    // folder says so: `cwd = "."`.
    let cwd = match &spec.cwd {
        Some(c) => c.clone(),
        None => exe.parent().map_or_else(|| grant.env.temp.clone(), Path::to_path_buf),
    };
    let started = create(&exe, &line, &cwd)?;

    let max = spec.max_output.clamp(1, limits::EXEC_MAX_OUTPUT);
    let limiter = Arc::new(Mutex::new(Limiter { window: Instant::now(), count: 0, dropped: 0 }));
    let make_mode = |_: bool| {
        if stream {
            Sinkmode::Lines(Stream { sink: sink.clone(), id, owner, mode: mode.clone(), limiter: limiter.clone() })
        } else {
            Sinkmode::Collect { max }
        }
    };
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let reader = |pipe: Owned, stderr: bool| {
        let m = make_mode(stderr);
        let done = done_tx.clone();
        std::thread::Builder::new().name("rsb-exec-read".into()).spawn(move || {
            let c = read_pipe(pipe, stderr, m);
            let _ = done.send(());
            c
        })
    };
    let r_out = reader(started.out, false).map_err(|e| IoError::Spawn(format!("reader: {e}")))?;
    let r_err = reader(started.err, true).map_err(|e| IoError::Spawn(format!("reader: {e}")))?;
    drop(done_tx);

    let timeout = match spec.timeout {
        Some(t) => Some(t.min(limits::EXEC_MAX_TIMEOUT)),
        None if stream => None,
        None => Some(limits::EXEC_DEFAULT_TIMEOUT),
    };
    let wait_ms = timeout.map_or(u32::MAX, |t| t.as_millis().min(u128::from(u32::MAX - 1)) as u32);
    // SAFETY: both handles are live: the process handle and the cancel event.
    let waited = unsafe { WaitForMultipleObjects(&[started.process.get(), cancel.get()], false, wait_ms) };
    let exited = waited == WAIT_OBJECT_0;
    let timed_out = waited == WAIT_TIMEOUT;
    if !exited {
        // SAFETY: the job is live; killing it ends the program and everything it started.
        let _ = unsafe { TerminateJobObject(started.job.get(), 1) };
    } else {
        // Let the readers reach end of file; a descendant that kept the pipe open is killed.
        let mut finished = 0;
        let end = Instant::now() + READER_GRACE;
        while finished < 2 {
            match done_rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                Ok(()) => finished += 1,
                Err(_) => break,
            }
        }
        if finished < 2 {
            // SAFETY: as above.
            let _ = unsafe { TerminateJobObject(started.job.get(), 1) };
        }
    }
    let out = r_out.join().unwrap_or(Collected { bytes: Vec::new(), truncated: false });
    let err = r_err.join().unwrap_or(Collected { bytes: Vec::new(), truncated: false });

    let code = if exited {
        let mut code = 0u32;
        // SAFETY: the process handle is live and `code` a live out pointer.
        unsafe { GetExitCodeProcess(started.process.get(), &mut code) }.ok().map(|()| code as i32)
    } else {
        None
    };
    let dropped = limiter.lock().unwrap_or_else(|e| e.into_inner()).dropped;
    if dropped > 0 {
        log::warn!("{}: dropped {dropped} output lines over {} a second", grant.widget, limits::EXEC_MAX_LINES_PER_SEC);
    }
    Ok(ExecResult {
        code,
        stdout: String::from_utf8_lossy(&out.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&err.bytes).into_owned(),
        timed_out,
        truncated: out.truncated || err.truncated,
    })
}

#[cfg(test)]
mod tests {
    use rsb_core::io::split_command_line;

    use super::*;

    #[test]
    fn quoting_round_trips_through_the_windows_rules() {
        let args = [
            "plain",
            "a b",
            "say \"hi\"",
            "back\\",
            "tail\\\\\"x",
            "",
            "tab\there",
            "C:\\Program Files\\x y\\",
            "\\\\",
        ];
        let line =
            std::iter::once("prog".to_string()).chain(args.iter().map(|a| quote_arg(a))).collect::<Vec<_>>().join(" ");
        let words = split_command_line(&line);
        assert_eq!(&words[1..], &args.map(String::from)[..]);
    }

    #[test]
    fn a_name_is_looked_up_on_path_only() {
        assert!(matches!(
            resolve_program(&ProgramRef::Name("definitely-not-a-program-xyz".into())),
            Err(IoError::Spawn(_))
        ));
        assert!(matches!(resolve_program(&ProgramRef::Path("relative.exe".into())), Err(IoError::BadRequest(_))));
        assert!(matches!(resolve_program(&ProgramRef::Path("C:\\x\\y.bat".into())), Err(IoError::BadRequest(_))));
    }
}
