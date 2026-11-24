//! komorebi through its documented subscribe pipe: we create a named pipe
//! server, ask `komorebic subscribe-pipe` to make komorebi connect to it,
//! and read one JSON notification per handled event
//! (`{"event":{...},"state":{...}}`). Only komorebi's command-line client
//! and pipe protocol are used.
//!
//! The pipe is created with `FILE_FLAG_FIRST_PIPE_INSTANCE` (a squatter on
//! the name makes us fail instead of sharing it), `PIPE_REJECT_REMOTE_CLIENTS`
//! and a protected DACL that names only our user. Each JSON object is
//! capped at `KOMOREBI_MAX_OBJECT`.

use std::os::windows::process::CommandExt;
use std::sync::Arc;
use std::time::Duration;

use rsb_core::services::ServiceStatus;
use serde_json::Value as Json;
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, ResetEvent, WaitForMultipleObjects};
use windows::core::HSTRING;

use super::glaze::End;
use super::model::from_komorebi;
use super::{Shared, publish_model};
use crate::win::{Owned, current_sid_string, manual_event};
use crate::{KOMOREBI_MAX_OBJECT, Publisher};

/// How long komorebi has to connect after we ask it to.
const CONNECT_WITHIN: Duration = Duration::from_secs(10);
/// An idle pipe is checked this often (is komorebi still running?).
const CHECK_EVERY: Duration = Duration::from_secs(60);
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the service does to komorebi besides reading its pipe. Behind a
/// trait so tests can stand in for `komorebic` and the process list.
pub trait KomorebiControl: Send + Sync {
    /// Is komorebi running?
    fn running(&self) -> bool;
    /// Make komorebi connect to the pipe `\\.\pipe\<pipe>` and notify us.
    fn subscribe(&self, pipe: &str) -> Result<(), String>;
    fn unsubscribe(&self, pipe: &str);
    /// Switch to the workspace with this name.
    fn focus_named(&self, name: &str) -> Result<(), String>;
}

/// The real thing: the process list and `komorebic` on PATH.
pub(crate) struct RealControl {
    pub komorebic: String,
}

impl RealControl {
    fn run(&self, args: &[&str], timeout: Duration) -> Result<(), String> {
        use rsb_core::io::ProgramRef;
        let exe = crate::ioreq::exec::resolve_program(&ProgramRef::Name(self.komorebic.clone()))
            .map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new(exe)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .map_err(|_| "komorebic could not be started".to_string())?;
        // Bounded wait on a helper thread, so nothing here ever polls or blocks for long.
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let r = child.wait();
            let _ = tx.send(());
            (child, r)
        });
        match rx.recv_timeout(timeout) {
            Ok(()) => match waiter.join() {
                Ok((_, Ok(status))) if status.success() => Ok(()),
                _ => Err("komorebic reported a failure".into()),
            },
            Err(_) => {
                // Not finished: it keeps running on its own; nothing waits for it any more.
                Err("komorebic did not answer in time".into())
            }
        }
    }
}

impl KomorebiControl for RealControl {
    fn running(&self) -> bool {
        // SAFETY: a plain snapshot of the process list; the handle is closed below.
        let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return false };
        let snap = Owned(snap);
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        // SAFETY: `entry` has its size set; `snap` is a live snapshot.
        let mut more = unsafe { Process32FirstW(snap.get(), &mut entry) }.is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("komorebi.exe") {
                return true;
            }
            // SAFETY: as above.
            more = unsafe { Process32NextW(snap.get(), &mut entry) }.is_ok();
        }
        false
    }

    fn subscribe(&self, pipe: &str) -> Result<(), String> {
        self.run(&["subscribe-pipe", pipe], SUBSCRIBE_TIMEOUT)
    }

    fn unsubscribe(&self, pipe: &str) {
        let _ = self.run(&["unsubscribe-pipe", pipe], SUBSCRIBE_TIMEOUT);
    }

    fn focus_named(&self, name: &str) -> Result<(), String> {
        if name.starts_with('-') {
            return Err("a workspace name cannot start with '-'".into());
        }
        self.run(&["focus-named-workspace", name], SUBSCRIBE_TIMEOUT)
    }
}

// --------------------------------------------------------------------- pipe

/// The first instance of our pipe: inbound, byte mode, overlapped, local
/// clients only, our user only.
fn create_pipe(name: &str) -> Result<Owned, String> {
    let sddl = format!("D:P(A;;GA;;;{})", current_sid_string()?);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the SDDL is a valid string; the descriptor is LocalAlloc'd and freed below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|e| format!("security descriptor: {e}"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let mode = NAMED_PIPE_MODE(PIPE_TYPE_BYTE.0 | PIPE_READMODE_BYTE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0);
    let full = format!("\\\\.\\pipe\\{name}");
    // SAFETY: the name and attributes outlive the call; the pipe keeps its own copy of the descriptor.
    let pipe = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(full),
            FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_INBOUND.0 | FILE_FLAG_OVERLAPPED.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0),
            mode,
            1,
            0,
            64 * 1024,
            0,
            Some(&attributes),
        )
    };
    let created = if pipe.is_invalid() { Err(windows::core::Error::from_thread()) } else { Ok(pipe) };
    // SAFETY: frees the descriptor allocated above, once.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    created.map(Owned).map_err(|e| format!("the komorebi pipe could not be created ({e})"))
}

#[derive(Debug, PartialEq)]
enum IoEnd {
    Stopped,
    TimedOut,
    Broken,
    Failed(String),
}

/// Runs one overlapped call, waiting on `event` and `stop` for at most
/// `timeout`. A call still pending at the end is cancelled and waited out.
///
/// # Safety
/// `start` must issue exactly one overlapped operation on `handle` with the
/// OVERLAPPED it is given, and everything it points at must outlive this call.
unsafe fn overlapped(
    handle: HANDLE,
    event: &Owned,
    stop: &Owned,
    timeout: Option<Duration>,
    start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
) -> Result<u32, IoEnd> {
    // SAFETY: `event` is a live event.
    let _ = unsafe { ResetEvent(event.get()) };
    let mut ov = OVERLAPPED { hEvent: event.get(), ..Default::default() };
    match start(&mut ov) {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
        Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(0),
        Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => return Err(IoEnd::Broken),
        Err(e) => return Err(IoEnd::Failed(e.to_string())),
    }
    let ms = timeout.map_or(u32::MAX, |t| t.as_millis().min(u128::from(u32::MAX - 1)) as u32);
    crate::count_source_wake();
    // SAFETY: both handles are live events.
    let waited = unsafe { WaitForMultipleObjects(&[event.get(), stop.get()], false, ms) };
    if waited != WAIT_OBJECT_0 {
        // SAFETY: cancels our own pending call, then waits until the kernel has left `ov`.
        unsafe {
            let _ = CancelIoEx(handle, Some(&ov));
            let mut ignored = 0u32;
            let _ = GetOverlappedResult(handle, &ov, &mut ignored, true);
        }
        return Err(if waited.0 == WAIT_OBJECT_0.0 + 1 { IoEnd::Stopped } else { IoEnd::TimedOut });
    }
    let mut transferred = 0u32;
    // SAFETY: the operation finished (the event is set), so this only reads its result.
    match unsafe { GetOverlappedResult(handle, &ov, &mut transferred, false) } {
        Ok(()) => Ok(transferred),
        Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Err(IoEnd::Broken),
        Err(e) => Err(IoEnd::Failed(e.to_string())),
    }
}

// ------------------------------------------------------------------- framer

/// Splits a byte stream into top-level JSON objects, whether or not they are
/// separated by newlines, never holding more than `max` bytes of one object.
pub(crate) struct Framer {
    buf: Vec<u8>,
    depth: usize,
    in_string: bool,
    escaped: bool,
    started: bool,
    max: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TooLarge;

impl Framer {
    pub(crate) fn new(max: usize) -> Framer {
        Framer { buf: Vec::new(), depth: 0, in_string: false, escaped: false, started: false, max }
    }

    /// Feeds bytes; returns every complete object. `Err` when one grows over the cap.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, TooLarge> {
        let mut done = Vec::new();
        for &b in bytes {
            if !self.started {
                if b == b'{' {
                    self.started = true;
                    self.depth = 1;
                    self.buf.clear();
                    self.buf.push(b);
                }
                // Anything between objects (newlines, spaces) is skipped.
                continue;
            }
            self.buf.push(b);
            if self.buf.len() > self.max {
                return Err(TooLarge);
            }
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if b == b'\\' {
                    self.escaped = true;
                } else if b == b'"' {
                    self.in_string = false;
                }
                continue;
            }
            match b {
                b'"' => self.in_string = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        self.started = false;
                        done.push(std::mem::take(&mut self.buf));
                    }
                }
                _ => {}
            }
        }
        Ok(done)
    }
}

/// One notification object into the model (`state` inside, or a bare state).
fn handle_object(bytes: &[u8], shared: &Arc<Shared>, out: &Publisher) {
    let Ok(json) = serde_json::from_slice::<Json>(bytes) else {
        log::debug!("workspaces: a komorebi object that is not JSON was ignored");
        return;
    };
    let state = json.get("state").or_else(|| json.get("monitors").map(|_| &json));
    if let Some(state) = state {
        publish_model(shared, out, from_komorebi(state), ServiceStatus::Ok);
    }
}

/// One pipe, from creation to loss or stop.
pub(crate) fn run_session(
    shared: &Arc<Shared>,
    out: &Publisher,
    stop: &Owned,
    control: &Arc<dyn KomorebiControl>,
    pipe_name: &str,
) -> End {
    let pipe = match create_pipe(pipe_name) {
        Ok(p) => p,
        Err(e) => return End::Lost(e),
    };
    let event = match manual_event() {
        Ok(e) => e,
        Err(e) => return End::Lost(e),
    };
    if let Err(e) = control.subscribe(pipe_name) {
        return End::Lost(e);
    }
    // komorebi connects as the client.
    // SAFETY: one ConnectNamedPipe on our pipe with the given OVERLAPPED.
    match unsafe {
        overlapped(pipe.get(), &event, stop, Some(CONNECT_WITHIN), |ov| ConnectNamedPipe(pipe.get(), Some(ov)))
    } {
        Ok(_) => {}
        Err(IoEnd::Stopped) => {
            control.unsubscribe(pipe_name);
            return End::Stopped;
        }
        Err(IoEnd::TimedOut) => return End::Lost("komorebi did not connect to the pipe".into()),
        Err(_) => return End::Lost("the komorebi pipe failed while waiting for a connection".into()),
    }
    log::info!("workspaces: connected to komorebi");

    let mut framer = Framer::new(KOMOREBI_MAX_OBJECT);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // SAFETY: one ReadFile on our pipe with the given OVERLAPPED; `buf` outlives the call.
        let read = unsafe {
            overlapped(pipe.get(), &event, stop, Some(CHECK_EVERY), |ov| {
                ReadFile(pipe.get(), Some(&mut buf[..]), None, Some(ov))
            })
        };
        match read {
            Ok(0) => return End::Lost("komorebi closed the pipe".into()),
            Ok(n) => match framer.push(&buf[..n as usize]) {
                Ok(objects) => {
                    for o in objects {
                        handle_object(&o, shared, out);
                    }
                }
                Err(TooLarge) => {
                    log::warn!(
                        "workspaces: a komorebi object over {KOMOREBI_MAX_OBJECT} bytes was refused; dropping the pipe"
                    );
                    return End::Lost("an object over the size limit was refused".into());
                }
            },
            Err(IoEnd::Stopped) => {
                control.unsubscribe(pipe_name);
                return End::Stopped;
            }
            Err(IoEnd::TimedOut) => {
                // A quiet minute: is komorebi still there?
                if control.running() {
                    out.alive();
                } else {
                    return End::Lost("komorebi is no longer running".into());
                }
            }
            Err(IoEnd::Broken) => return End::Lost("komorebi closed the pipe".into()),
            Err(IoEnd::Failed(_)) => return End::Lost("the komorebi pipe failed".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(chunks: &[&[u8]], max: usize) -> Result<Vec<String>, TooLarge> {
        let mut f = Framer::new(max);
        let mut out = Vec::new();
        for c in chunks {
            out.extend(f.push(c)?.into_iter().map(|o| String::from_utf8(o).unwrap()));
        }
        Ok(out)
    }

    #[test]
    fn objects_split_with_or_without_newlines() {
        let r = frames(&[b"{\"a\":1}\n{\"b\":[1,2]}{\"c\":{}}"], 1000).unwrap();
        assert_eq!(r, vec!["{\"a\":1}", "{\"b\":[1,2]}", "{\"c\":{}}"]);
    }

    #[test]
    fn an_object_may_arrive_in_pieces() {
        let r = frames(&[b"{\"a\":", b"\"x}\\\"", b"y\"}\n{\"b\"", b":2}"], 1000).unwrap();
        assert_eq!(
            r,
            vec!["{\"a\":\"x}\\\"y\"}", "{\"b\":2}"],
            "braces and escaped quotes inside strings do not end an object"
        );
    }

    #[test]
    fn an_object_over_the_cap_is_refused() {
        let big = format!("{{\"a\":\"{}\"}}", "x".repeat(2000));
        assert_eq!(frames(&[big.as_bytes()], 1000), Err(TooLarge));
        let fits = format!("{{\"a\":\"{}\"}}", "x".repeat(900));
        assert!(frames(&[fits.as_bytes()], 1000).is_ok());
    }

    #[test]
    fn the_cap_counts_one_object_not_the_stream() {
        let one = format!("{{\"a\":\"{}\"}}\n", "x".repeat(400));
        let many = one.repeat(20);
        assert_eq!(frames(&[many.as_bytes()], 1000).unwrap().len(), 20);
    }

    fn pipe_dacl(pipe: &Owned) -> String {
        use windows::Win32::Foundation::HLOCAL;
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo, SE_KERNEL_OBJECT,
        };
        use windows::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: a live pipe handle and a live out pointer; the descriptor is freed below.
        unsafe {
            GetSecurityInfo(
                pipe.get(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                Some(&mut sd),
            )
        }
        .ok()
        .expect("read the pipe's security");
        let mut text = windows::core::PWSTR::null();
        // SAFETY: `sd` came from GetSecurityInfo; `text` receives a LocalAlloc'd string.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, 1, DACL_SECURITY_INFORMATION, &mut text, None)
        }
        .expect("convert to text");
        // SAFETY: `text` is a valid NUL-terminated string until freed.
        let out = unsafe { text.to_string() }.unwrap();
        // SAFETY: both were allocated by the calls above and are freed once.
        unsafe {
            LocalFree(Some(HLOCAL(text.0.cast())));
            LocalFree(Some(HLOCAL(sd.0)));
        }
        out
    }

    #[test]
    fn the_pipe_is_ours_alone_and_cannot_be_squatted() {
        let name = format!("rsb-unit-komorebi-{}", std::process::id());
        let pipe = create_pipe(&name).expect("the first instance is created");
        // A second server on the same name (someone racing us) fails instead of sharing it.
        assert!(create_pipe(&name).is_err(), "FILE_FLAG_FIRST_PIPE_INSTANCE refuses a second server");
        let sddl = pipe_dacl(&pipe);
        let sid = current_sid_string().unwrap();
        // Protected (no inherited entries), one allow entry, our user; Windows writes GENERIC_ALL as FA.
        assert_eq!(sddl, format!("D:P(A;;FA;;;{sid})"));
    }
}
