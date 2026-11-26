//! The command pipe: `\\.\pipe\RSB-<user SID>-<session id>`.
//!
//! Server rules (the pipe is a trust boundary even on one PC):
//! - the DACL grants the current user's SID only (explicit descriptor,
//!   protected, no inherited entries, built from the token's SID);
//! - `PIPE_REJECT_REMOTE_CLIENTS` and `FILE_FLAG_FIRST_PIPE_INSTANCE`: if
//!   someone else already owns the name the server refuses to start instead
//!   of sharing a name with a squatter;
//! - message mode, a request over `MAX_REQUEST_BYTES` is refused with an
//!   error reply, a reply never exceeds `MAX_REPLY_BYTES`;
//! - one request per connection; a client that sends nothing for 2 s is
//!   dropped, so one stuck client cannot block the next;
//! - overlapped I/O with a stop event, so quitting never hangs in
//!   `ConnectNamedPipe`.
//!
//! Client rules: connect with a 2 s wait, then check that the server runs in
//! our session and as our user (`GetNamedPipeServerProcessId`, then its
//! session id and token SID) before sending anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rsb_core::ipc::{Command, MAX_REPLY_BYTES, MAX_REQUEST_BYTES, Reply};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree, WAIT_EVENT, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, NAMED_PIPE_MODE,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT, SetNamedPipeHandleState,
    WaitNamedPipeW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{HRESULT, HSTRING, PWSTR};

/// A client that sends nothing for this long is dropped.
const READ_TIMEOUT_MS: u32 = 2_000;
/// Longest the server waits to hand a reply to a client.
const WRITE_TIMEOUT_MS: u32 = 2_000;
/// Longest the server waits for the client to close after the reply.
const CLOSE_WAIT_MS: u32 = 1_000;
/// Longest a client waits for a busy pipe.
const BUSY_WAIT_MS: u32 = 2_000;
/// How long `stop` lets a request in progress finish (a `quit` reply must
/// reach its client before the pipe goes away).
const STOP_GRACE: Duration = Duration::from_millis(500);

/// Error text of `send` when nobody owns the pipe.
pub const NO_BAR: &str = "no bar running";

/// Answers one decoded command. Runs on the pipe thread: it may wait for
/// other threads, but only with its own short timeouts.
pub type Handler = Arc<dyn Fn(Command) -> Reply + Send + Sync>;

/// An owned kernel handle, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was created by this process and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// A handle value moved to the pipe thread. Kernel handles are
/// process-wide, so using one on another thread is sound.
#[derive(Clone, Copy)]
struct SendHandle(isize);

impl SendHandle {
    fn new(h: HANDLE) -> SendHandle {
        SendHandle(h.0 as isize)
    }

    fn get(self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }
}

fn win32_code(code: windows::Win32::Foundation::WIN32_ERROR) -> HRESULT {
    HRESULT::from_win32(code.0)
}

/// The current user's SID as text (`S-1-5-21-...`), read from the process token.
fn current_sid_string() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle is always valid; `token` receives a new handle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("OpenProcessToken: {e}"))?;
    sid_string_of(&Owned(token))
}

/// The user SID of the process with this id. Fails (rather than guessing)
/// when the process or its token cannot be opened, which is what happens
/// for another account's process.
fn process_sid_string(pid: u32) -> Result<String, String> {
    // SAFETY: plain flag and id; the returned handle is owned below.
    let process = Owned(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .map_err(|e| format!("OpenProcess: {e}"))?,
    );
    let mut token = HANDLE::default();
    // SAFETY: `process` is a live handle; `token` receives a new handle.
    unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) }.map_err(|e| format!("OpenProcessToken: {e}"))?;
    sid_string_of(&Owned(token))
}

fn sid_string_of(token: &Owned) -> Result<String, String> {
    let mut len = 0u32;
    // SAFETY: a size query; the call is expected to fail with the needed length.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
    if len == 0 {
        return Err("GetTokenInformation reported no size".to_string());
    }
    // u64 elements so the buffer is aligned for TOKEN_USER's pointer field.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes and is suitably aligned.
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len) }
        .map_err(|e| format!("GetTokenInformation: {e}"))?;
    // SAFETY: the call above filled `buf` with a TOKEN_USER whose SID points into the same buffer.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid for the life of `buf`; on success `text` is a LocalAlloc'd string.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|e| format!("ConvertSidToStringSidW: {e}"))?;
    // SAFETY: `text` is a valid NUL-terminated string until freed below.
    let out = unsafe { text.to_string() }.map_err(|e| e.to_string());
    // SAFETY: frees the string allocated by ConvertSidToStringSidW, once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    out
}

fn session_id() -> Result<u32, String> {
    let mut session = 0u32;
    // SAFETY: plain process id and a live out pointer.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }
        .map_err(|e| format!("ProcessIdToSessionId: {e}"))?;
    Ok(session)
}

/// Name of the environment variable that gives a test run its own pipe and
/// instance mutex, so a test never reaches (or collides with) a real bar.
pub const TEST_TAG_VAR: &str = "RSB_TEST_TAG";

/// The tag from the environment: letters, digits and `-` only.
pub fn test_tag() -> Option<String> {
    let tag = std::env::var(TEST_TAG_VAR).ok()?;
    (!tag.is_empty() && tag.len() <= 32 && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then_some(tag)
}

/// The pipe path for the current user and session.
pub fn pipe_name() -> Result<String, String> {
    pipe_name_tagged(test_tag().as_deref())
}

pub fn pipe_name_tagged(tag: Option<&str>) -> Result<String, String> {
    let suffix = tag.map(|t| format!("-{t}")).unwrap_or_default();
    Ok(format!(r"\\.\pipe\RSB-{}-{}{suffix}", current_sid_string()?, session_id()?))
}

/// SDDL of the pipe: a protected DACL with one entry, full access for `sid`.
/// No owner, group or inherited entries: nobody else is named anywhere.
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// The running server thread.
pub struct PipeServer {
    thread: Option<std::thread::JoinHandle<()>>,
    stop_event: SendHandle,
    busy: Arc<AtomicBool>,
}

impl PipeServer {
    /// Creates the first pipe instance on the calling thread (so a squatted
    /// name fails the start loudly), then serves on a thread named "rsb-pipe".
    pub fn start(handler: Handler) -> Result<PipeServer, String> {
        PipeServer::start_named(&pipe_name()?, handler)
    }

    pub fn start_named(name: &str, handler: Handler) -> Result<PipeServer, String> {
        let sddl = pipe_sddl(&current_sid_string()?);
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
        .map_err(|e| format!("ConvertStringSecurityDescriptorToSecurityDescriptorW: {e}"))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let mode =
            NAMED_PIPE_MODE(PIPE_TYPE_MESSAGE.0 | PIPE_READMODE_MESSAGE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0);
        // SAFETY: `attributes` and the descriptor outlive the call; the pipe keeps its own copy.
        let pipe = unsafe {
            CreateNamedPipeW(
                &HSTRING::from(name),
                FILE_FLAGS_AND_ATTRIBUTES(
                    PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_OVERLAPPED.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0,
                ),
                mode,
                1,
                MAX_REPLY_BYTES as u32,
                (MAX_REQUEST_BYTES + 1) as u32,
                0,
                Some(&attributes),
            )
        };
        let created = if pipe.is_invalid() { Err(windows::core::Error::from_thread()) } else { Ok(pipe) };
        // SAFETY: frees the descriptor allocated above, once.
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        let pipe = Owned(created.map_err(|e| format!("CreateNamedPipeW (another process may own the name): {e}"))?);

        // SAFETY: a manual-reset, unnamed event.
        let stop = unsafe { CreateEventW(None, true, false, None) }.map_err(|e| format!("CreateEventW: {e}"))?;
        let stop_handle = SendHandle::new(stop);
        let pipe_handle = SendHandle::new(pipe.0);
        std::mem::forget(pipe); // the thread owns it from here
        let busy = Arc::new(AtomicBool::new(false));
        let thread_busy = Arc::clone(&busy);
        let thread = std::thread::Builder::new()
            .name("rsb-pipe".into())
            .spawn(move || serve(pipe_handle, stop_handle, &handler, &thread_busy))
            .map_err(|e| {
                // SAFETY: both handles are ours and unused by anyone else.
                unsafe {
                    let _ = CloseHandle(pipe_handle.get());
                    let _ = CloseHandle(stop);
                }
                format!("could not start the pipe thread: {e}")
            })?;
        Ok(PipeServer { thread: Some(thread), stop_event: stop_handle, busy })
    }

    /// Lets a request in progress finish (up to half a second), signals the
    /// stop event and joins the thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else { return };
        let until = Instant::now() + STOP_GRACE;
        while self.busy.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        // SAFETY: the event is open until the close below.
        let _ = unsafe { SetEvent(self.stop_event.get()) };
        let _ = thread.join();
        // SAFETY: the thread is gone, so nothing waits on the event any more.
        let _ = unsafe { CloseHandle(self.stop_event.get()) };
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Why an overlapped call did not finish normally.
#[derive(Debug)]
enum IoFail {
    /// The stop event fired.
    Stopped,
    /// No completion within the time allowed; the call was cancelled.
    TimedOut,
    /// A message longer than the buffer.
    MoreData,
    Os(String),
}

/// Runs one overlapped call to completion, waiting on `event` (and `stop`
/// when given) for at most `timeout_ms`. A call still pending at the end is
/// cancelled and waited out, so the buffers it uses may be dropped safely.
///
/// # Safety
/// `start` must issue exactly one overlapped operation on `handle` using the
/// OVERLAPPED it is given, and everything that operation points at must stay
/// alive until this function returns.
unsafe fn overlapped(
    handle: HANDLE,
    event: HANDLE,
    stop: Option<HANDLE>,
    timeout_ms: u32,
    start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
) -> Result<u32, IoFail> {
    // SAFETY: `event` is a valid event handle owned by the caller.
    let _ = unsafe { ResetEvent(event) };
    let mut ov = OVERLAPPED { hEvent: event, ..Default::default() };
    match start(&mut ov) {
        Ok(()) => {}
        Err(e) if e.code() == win32_code(ERROR_IO_PENDING) => {}
        Err(e) if e.code() == win32_code(ERROR_PIPE_CONNECTED) => return Ok(0),
        Err(e) if e.code() == win32_code(ERROR_MORE_DATA) => return Err(IoFail::MoreData),
        Err(e) => return Err(IoFail::Os(e.to_string())),
    }
    let mut events = vec![event];
    events.extend(stop);
    // SAFETY: the handles stay valid for the call.
    let waited: WAIT_EVENT = unsafe { WaitForMultipleObjects(&events, false, timeout_ms) };
    if waited != WAIT_OBJECT_0 {
        // SAFETY: cancels our own pending call, then waits until it has left `ov`.
        unsafe {
            let _ = CancelIoEx(handle, Some(&ov));
            let mut ignored = 0u32;
            let _ = GetOverlappedResult(handle, &ov, &mut ignored, true);
        }
        return Err(if waited.0 == WAIT_OBJECT_0.0 + 1 { IoFail::Stopped } else { IoFail::TimedOut });
    }
    let mut transferred = 0u32;
    // SAFETY: the operation finished (the event is set), so this only reads its result.
    match unsafe { GetOverlappedResult(handle, &ov, &mut transferred, false) } {
        Ok(()) => Ok(transferred),
        Err(e) if e.code() == win32_code(ERROR_MORE_DATA) => Err(IoFail::MoreData),
        Err(e) => Err(IoFail::Os(e.to_string())),
    }
}

/// Reads one message into `buf`; a longer one is `MoreData`.
fn read_message(
    pipe: HANDLE,
    event: HANDLE,
    stop: Option<HANDLE>,
    timeout_ms: u32,
    buf: &mut [u8],
) -> Result<usize, IoFail> {
    let limit = buf.len() - 1;
    // SAFETY: `buf` outlives the call; one ReadFile on `pipe` with the given OVERLAPPED.
    let n =
        unsafe { overlapped(pipe, event, stop, timeout_ms, |ov| ReadFile(pipe, Some(&mut buf[..]), None, Some(ov))) }?;
    if n as usize > limit { Err(IoFail::MoreData) } else { Ok(n as usize) }
}

fn write_message(
    pipe: HANDLE,
    event: HANDLE,
    stop: Option<HANDLE>,
    timeout_ms: u32,
    bytes: &[u8],
) -> Result<(), IoFail> {
    // SAFETY: `bytes` outlives the call; one WriteFile on `pipe` with the given OVERLAPPED.
    unsafe { overlapped(pipe, event, stop, timeout_ms, |ov| WriteFile(pipe, Some(bytes), None, Some(ov))) }.map(|_| ())
}

/// Body of the pipe thread.
fn serve(pipe: SendHandle, stop: SendHandle, handler: &Handler, busy: &AtomicBool) {
    let pipe = Owned(pipe.get());
    let stop = stop.get();
    // SAFETY: a manual-reset, unnamed event used for every overlapped call here.
    let Ok(event) = (unsafe { CreateEventW(None, true, false, None) }) else {
        log::error!("pipe: could not create an event; the command pipe is off");
        return;
    };
    let event = Owned(event);
    loop {
        // SAFETY: `stop` is open for as long as the owner has not joined this thread.
        if unsafe { WaitForSingleObject(stop, 0) } == WAIT_OBJECT_0 {
            break;
        }
        // SAFETY: one ConnectNamedPipe on our pipe with the given OVERLAPPED.
        let connected =
            unsafe { overlapped(pipe.0, event.0, Some(stop), u32::MAX, |ov| ConnectNamedPipe(pipe.0, Some(ov))) };
        match connected {
            Ok(_) => {}
            Err(IoFail::Stopped) => break,
            Err(other) => {
                log::warn!("pipe: connect failed: {other:?}");
                // SAFETY: plain call on our handle; fails harmlessly when not connected.
                let _ = unsafe { DisconnectNamedPipe(pipe.0) };
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }
        busy.store(true, Ordering::Release);
        let reply = serve_one(pipe.0, event.0, stop, handler);
        let stopped = match reply {
            Ok(bytes) => match write_message(pipe.0, event.0, Some(stop), WRITE_TIMEOUT_MS, bytes.as_bytes()) {
                Ok(()) => {
                    // Wait for the client to close (it does after reading the
                    // reply) so DisconnectNamedPipe cannot discard unread data.
                    // What is left of an oversized request is drained first.
                    let mut scratch = [0u8; 256];
                    let mut stopped = false;
                    for _ in 0..32 {
                        match read_message(pipe.0, event.0, Some(stop), CLOSE_WAIT_MS, &mut scratch) {
                            Ok(_) | Err(IoFail::MoreData) => continue,
                            Err(IoFail::Stopped) => stopped = true,
                            Err(_) => {}
                        }
                        break;
                    }
                    stopped
                }
                Err(IoFail::Stopped) => true,
                Err(other) => {
                    log::warn!("pipe: a reply was not delivered: {other:?}");
                    false
                }
            },
            Err(IoFail::Stopped) => true,
            Err(other) => {
                log::warn!("pipe: a client was dropped: {other:?}");
                false
            }
        };
        // SAFETY: plain call on our handle.
        let _ = unsafe { DisconnectNamedPipe(pipe.0) };
        busy.store(false, Ordering::Release);
        if stopped {
            break;
        }
    }
}

/// Reads of one request buffer spent discarding an oversized message (about 256 KiB).
const MAX_DRAIN_READS: usize = 64;

/// Reads and answers one request: the encoded reply to write.
fn serve_one(pipe: HANDLE, event: HANDLE, stop: HANDLE, handler: &Handler) -> Result<String, IoFail> {
    let mut buf = [0u8; MAX_REQUEST_BYTES + 1];
    let n = match read_message(pipe, event, Some(stop), READ_TIMEOUT_MS, &mut buf) {
        Ok(n) => n,
        Err(IoFail::MoreData) => {
            log::warn!("pipe: refused an oversized request");
            // The rest of the message is still in the pipe and the client is still writing it: read it
            // away (a bounded amount) or the client never gets this answer.
            for _ in 0..MAX_DRAIN_READS {
                match read_message(pipe, event, Some(stop), READ_TIMEOUT_MS, &mut buf) {
                    Err(IoFail::MoreData) => {}
                    _ => break,
                }
            }
            return Ok(Reply::Err(format!("the command is longer than {MAX_REQUEST_BYTES} bytes")).encode());
        }
        Err(other) => return Err(other),
    };
    match Command::decode(&buf[..n]) {
        Ok(command) => Ok(handler(command).encode()),
        Err(e) => {
            log::warn!("pipe: refused a request ({n} bytes): {e}");
            Ok(Reply::Err(e.to_string()).encode())
        }
    }
}

/// Why a command could not be delivered, mapped to exit codes by the caller.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// Nobody owns the pipe: exit 3.
    NoBar,
    /// Not our user or session, or no answer in time: exit 4.
    Refused(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NoBar => f.write_str(NO_BAR),
            SendError::Refused(why) => f.write_str(why),
        }
    }
}

/// Sends one command to the running bar and returns its reply.
pub fn send(command: &Command, timeout: Duration) -> Result<Reply, SendError> {
    send_to(&pipe_name().map_err(SendError::Refused)?, command, timeout)
}

pub fn send_to(name: &str, command: &Command, timeout: Duration) -> Result<Reply, SendError> {
    let refused = |m: String| SendError::Refused(m);
    let deadline = Instant::now() + timeout;
    let wide = HSTRING::from(name);
    let pipe = loop {
        // SAFETY: `wide` outlives the call. SQOS_PRESENT + IDENTIFICATION: the
        // server may learn who we are but never act as us.
        let opened = unsafe {
            CreateFileW(
                &wide,
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0 | SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                None,
            )
        };
        match opened {
            Ok(h) => break Owned(h),
            Err(e) if e.code() == win32_code(ERROR_FILE_NOT_FOUND) => return Err(SendError::NoBar),
            Err(e) if e.code() == win32_code(ERROR_PIPE_BUSY) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(refused("the bar's pipe stayed busy".into()));
                }
                let wait = left.as_millis().min(u128::from(BUSY_WAIT_MS)) as u32;
                // SAFETY: `wide` outlives the call.
                let _ = unsafe { WaitNamedPipeW(&wide, wait.max(1)) };
            }
            Err(e) => return Err(refused(format!("cannot open the bar's pipe: {e}"))),
        }
    };

    // The server must run in our session and as us; anything else squatting
    // the (predictable) name is refused before a byte is sent.
    let mut server_pid = 0u32;
    // SAFETY: `pipe` is a valid pipe handle; the out pointer is live.
    unsafe { GetNamedPipeServerProcessId(pipe.0, &mut server_pid) }
        .map_err(|e| refused(format!("GetNamedPipeServerProcessId: {e}")))?;
    let mut server_session = u32::MAX;
    // SAFETY: plain process id and a live out pointer.
    unsafe { ProcessIdToSessionId(server_pid, &mut server_session) }
        .map_err(|e| refused(format!("ProcessIdToSessionId: {e}")))?;
    if server_session != session_id().map_err(refused)? {
        return Err(refused("the pipe belongs to a process in another session".into()));
    }
    match process_sid_string(server_pid) {
        Ok(sid) if sid == current_sid_string().map_err(refused)? => {}
        Ok(_) => return Err(refused("the pipe belongs to a process of another user".into())),
        Err(e) => return Err(refused(format!("cannot check who owns the pipe ({e})"))),
    }

    if opens_a_popup(command) {
        allow_foreground(server_pid);
    }

    let mode = PIPE_READMODE_MESSAGE;
    // SAFETY: `mode` is a live local.
    unsafe { SetNamedPipeHandleState(pipe.0, Some(&mode), None, None) }
        .map_err(|e| refused(format!("SetNamedPipeHandleState: {e}")))?;

    // SAFETY: a manual-reset, unnamed event.
    let event =
        Owned(unsafe { CreateEventW(None, true, false, None) }.map_err(|e| refused(format!("CreateEventW: {e}")))?);
    let left_ms = |at: Instant| {
        at.saturating_duration_since(Instant::now()).as_millis().clamp(1, u128::from(u32::MAX - 1)) as u32
    };
    write_message(pipe.0, event.0, None, left_ms(deadline), command.encode().as_bytes())
        .map_err(|f| refused(describe(f)))?;
    let mut buf = vec![0u8; MAX_REPLY_BYTES + 1];
    let n = read_message(pipe.0, event.0, None, left_ms(deadline), &mut buf).map_err(|f| refused(describe(f)))?;
    Reply::decode(&buf[..n]).map_err(|e| refused(e.to_string()))
}

/// `rsb popup <name>` may open a popup that takes keyboard focus. Windows lets
/// a process take the foreground only when the process that had the user's
/// last input hands the right over, so the client (which a hotkey tool just
/// started) passes it to the bar it has just checked belongs to this user.
fn opens_a_popup(command: &Command) -> bool {
    matches!(
        command,
        Command::Popup { action: rsb_core::popup::PopupAction::Open | rsb_core::popup::PopupAction::Toggle, .. }
    )
}

fn allow_foreground(server_pid: u32) {
    // SAFETY: a plain process id; a failure only means the popup will not take focus.
    if let Err(e) = unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(server_pid) } {
        log::debug!("AllowSetForegroundWindow: {e}");
    }
}

fn describe(fail: IoFail) -> String {
    match fail {
        IoFail::Stopped => "stopped".to_string(),
        IoFail::TimedOut => "the bar did not answer in time".to_string(),
        IoFail::MoreData => "the reply is longer than the limit".to_string(),
        IoFail::Os(e) => e,
    }
}

/// The DACL of a live pipe as SDDL text, read from a client handle.
/// For the owner-only check in tests.
pub fn read_back_sddl(name: &str) -> Result<String, String> {
    let wide = HSTRING::from(name);
    // SAFETY: `wide` outlives the call. READ_CONTROL (inside GENERIC_READ) is what reading the DACL needs.
    let handle = Owned(
        unsafe {
            CreateFileW(
                &wide,
                GENERIC_READ.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0),
                None,
            )
        }
        .map_err(|e| format!("open the pipe: {e}"))?,
    );
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `handle` is open; the descriptor is LocalAlloc'd and freed below.
    let status = unsafe {
        GetSecurityInfo(
            handle.0,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    if status.0 != 0 {
        return Err(format!("GetSecurityInfo failed with {}", status.0));
    }
    let mut text = PWSTR::null();
    // SAFETY: `descriptor` came from GetSecurityInfo; `text` receives a LocalAlloc'd string.
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            None,
        )
    };
    // SAFETY: frees the descriptor allocated by GetSecurityInfo, once.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    converted.map_err(|e| format!("ConvertSecurityDescriptorToStringSecurityDescriptorW: {e}"))?;
    // SAFETY: `text` is a valid NUL-terminated string until freed below.
    let out = unsafe { text.to_string() }.map_err(|e| e.to_string());
    // SAFETY: frees the string allocated by the conversion, once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    out
}

/// The current user's SID text (for tests that compare it with the DACL).
pub fn current_sid() -> Result<String, String> {
    current_sid_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn test_name() -> String {
        format!(r"\\.\pipe\RSB-test-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
    }

    fn echo() -> Handler {
        Arc::new(|c: Command| Reply::Ok(format!("got {}", c.encode().trim_end())))
    }

    #[test]
    fn only_a_command_that_can_open_a_popup_passes_the_right_to_take_focus() {
        use rsb_core::popup::PopupAction::{Close, Open, Toggle};
        let popup = |action| Command::Popup { name: "launcher".into(), action };
        assert!(opens_a_popup(&popup(Open)));
        assert!(opens_a_popup(&popup(Toggle)));
        assert!(!opens_a_popup(&popup(Close)));
        assert!(!opens_a_popup(&Command::Toggle(None)) && !opens_a_popup(&Command::Reload));
    }

    #[test]
    fn a_popup_command_round_trips_through_a_real_pipe_and_the_client_passes_the_right() {
        // The server here is this process: the right to the foreground is passed to ourselves, which is harmless.
        let name = test_name();
        let server = PipeServer::start_named(&name, echo()).unwrap();
        let command = Command::Popup { name: "launcher".into(), action: rsb_core::popup::PopupAction::Open };
        assert_eq!(send_to(&name, &command, Duration::from_secs(5)), Ok(Reply::Ok("got popup launcher open".into())));
        server.stop();
    }

    #[test]
    fn the_name_carries_the_sid_and_the_session() {
        let name = pipe_name().unwrap();
        assert!(name.starts_with(r"\\.\pipe\RSB-S-1-"), "{name}");
        assert!(name.ends_with(&format!("-{}", session_id().unwrap())), "{name}");
    }

    #[test]
    fn the_sddl_names_one_sid_and_nothing_inherited() {
        let sddl = pipe_sddl("S-1-5-21-1-2-3-1001");
        assert_eq!(sddl, "D:P(A;;GA;;;S-1-5-21-1-2-3-1001)");
    }

    #[test]
    fn commands_round_trip_through_a_real_pipe() {
        let name = test_name();
        let server = PipeServer::start_named(&name, echo()).unwrap();
        for command in [Command::Reload, Command::Toggle(Some("top".into())), Command::Status] {
            let want = format!("got {}", command.encode().trim_end());
            assert_eq!(send_to(&name, &command, Duration::from_secs(5)), Ok(Reply::Ok(want)));
        }
        server.stop();
        assert_eq!(send_to(&name, &Command::Reload, Duration::from_millis(500)), Err(SendError::NoBar));
    }

    #[test]
    fn the_dacl_read_back_from_a_live_pipe_is_owner_only() {
        let name = test_name();
        let _server = PipeServer::start_named(&name, echo()).unwrap();
        let sddl = read_back_sddl(&name).unwrap();
        let sid = current_sid().unwrap();
        assert!(sddl.starts_with("D:P"), "protected, no inherited entries: {sddl}");
        assert_eq!(sddl.matches("(A;").count(), 1, "exactly one entry: {sddl}");
        assert!(sddl.contains(&sid) && !sddl.contains("(D;"), "{sddl}");
        assert!(!sddl.contains(";WD)") && !sddl.contains(";BU)") && !sddl.contains(";AU)"), "nobody else: {sddl}");
    }

    #[test]
    fn a_second_server_on_the_same_name_is_refused() {
        let name = test_name();
        let _first = PipeServer::start_named(&name, echo()).unwrap();
        let second = PipeServer::start_named(&name, echo());
        assert!(second.is_err_and(|e| e.contains("CreateNamedPipeW")));
    }

    #[test]
    fn raw_garbage_and_oversized_requests_get_an_error_reply() {
        let name = test_name();
        let _server = PipeServer::start_named(&name, echo()).unwrap();
        let exchange = |bytes: &[u8]| -> Reply {
            let wide = HSTRING::from(name.as_str());
            let deadline = Instant::now() + Duration::from_secs(5);
            let pipe = loop {
                // SAFETY: plain open of an existing pipe.
                let opened = unsafe {
                    CreateFileW(
                        &wide,
                        GENERIC_READ.0 | GENERIC_WRITE.0,
                        FILE_SHARE_NONE,
                        None,
                        OPEN_EXISTING,
                        FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0),
                        None,
                    )
                };
                match opened {
                    Ok(h) => break Owned(h),
                    Err(e) if e.code() == win32_code(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                        // SAFETY: `wide` outlives the call.
                        let _ = unsafe { WaitNamedPipeW(&wide, 200) };
                    }
                    Err(e) => panic!("open: {e}"),
                }
            };
            let mode = PIPE_READMODE_MESSAGE;
            // SAFETY: `mode` is a live local.
            unsafe { SetNamedPipeHandleState(pipe.0, Some(&mode), None, None) }.unwrap();
            // SAFETY: a manual-reset, unnamed event.
            let event = Owned(unsafe { CreateEventW(None, true, false, None) }.unwrap());
            write_message(pipe.0, event.0, None, 5_000, bytes).unwrap();
            let mut buf = vec![0u8; MAX_REPLY_BYTES + 1];
            let n = read_message(pipe.0, event.0, None, 5_000, &mut buf).unwrap();
            Reply::decode(&buf[..n]).unwrap()
        };
        assert!(matches!(exchange(b"frobnicate\n"), Reply::Err(m) if m.contains("unknown command")));
        assert!(matches!(exchange(b"quit"), Reply::Err(_)), "no newline");
        assert!(matches!(exchange(b"reload\x00\n"), Reply::Err(_)));
        let big = vec![b'a'; MAX_REQUEST_BYTES + 100];
        assert!(matches!(exchange(&big), Reply::Err(m) if m.contains("longer than")));
        // A message far larger than the buffer still gets its answer, not a broken pipe.
        let huge = vec![b'a'; 60_000];
        assert!(matches!(exchange(&huge), Reply::Err(m) if m.contains("longer than")));
        // The server is still answering afterwards.
        assert_eq!(exchange(b"reload\n"), Reply::Ok("got reload".into()));
    }
}
