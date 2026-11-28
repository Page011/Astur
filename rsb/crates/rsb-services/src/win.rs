//! Small Win32 helpers shared by the services: owned handles, events, wide
//! strings, user and session identity.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Memory::{GetProcessHeap, HeapOptimizeResources, HeapSetInformation};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, SetEvent,
};
use windows::core::{PCWSTR, PWSTR};

/// A kernel handle closed on drop. Kernel handles are process-wide, so
/// moving one to another thread is sound.
pub(crate) struct Owned(pub HANDLE);

// SAFETY: a kernel handle is a process-wide token; every API used on it is thread-safe.
unsafe impl Send for Owned {}
// SAFETY: as above; shared use only waits on, signals or reads from the handle.
unsafe impl Sync for Owned {}

impl Owned {
    pub(crate) fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: the handle was opened by this process and is closed exactly once, here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Set while a trim is waiting, so a burst of requests makes one.
static TRIM_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A moment from now, asks Windows to hand the process heap's free pages back. A popup that held a long
/// list leaves a heap full of holes that stay committed for as long as the bar runs. Runs on its own
/// thread: the heap is locked while it works.
pub fn trim_heap_soon() {
    use std::sync::atomic::Ordering::SeqCst;
    if TRIM_PENDING.swap(true, SeqCst) {
        return;
    }
    let started = std::thread::Builder::new().name("rsb-heap-trim".into()).spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        TRIM_PENDING.store(false, SeqCst);
        // HEAP_OPTIMIZE_RESOURCES_INFORMATION { Version: 1, Flags: 0 }.
        let info: [u32; 2] = [1, 0];
        // SAFETY: the process heap handle is always valid and `info` outlives the call.
        let done = unsafe {
            HeapSetInformation(
                Some(GetProcessHeap().unwrap_or_default()),
                HeapOptimizeResources,
                Some(info.as_ptr().cast()),
                8,
            )
        };
        if let Err(e) = done {
            log::debug!("heap trim: {e}");
        }
    });
    if started.is_err() {
        TRIM_PENDING.store(false, SeqCst);
    }
}

/// An unnamed manual-reset event, initially not signalled.
pub(crate) fn manual_event() -> Result<Owned, String> {
    // SAFETY: no pointers are passed; the new handle is owned by the result.
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map(Owned).map_err(|e| format!("CreateEventW: {e}"))
}

/// An unnamed auto-reset event, initially not signalled.
pub(crate) fn auto_event() -> Result<Owned, String> {
    // SAFETY: no pointers are passed; the new handle is owned by the result.
    unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Owned).map_err(|e| format!("CreateEventW: {e}"))
}

pub(crate) fn signal(event: &Owned) {
    // SAFETY: the handle is a live event owned by `event`.
    let _ = unsafe { SetEvent(event.get()) };
}

/// NUL-terminated UTF-16.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The user SID of a token, as text.
fn token_sid_string(token: &Owned) -> Result<String, String> {
    let mut len = 0u32;
    // SAFETY: a size query; the call is expected to fail with the needed length.
    let _ = unsafe { GetTokenInformation(token.get(), TokenUser, None, 0, &mut len) };
    if len == 0 {
        return Err("GetTokenInformation reported no size".into());
    }
    // u64 elements so the buffer is aligned for TOKEN_USER's pointer field.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes and is suitably aligned.
    unsafe { GetTokenInformation(token.get(), TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len) }
        .map_err(|e| format!("GetTokenInformation: {e}"))?;
    // SAFETY: the call above filled `buf` with a TOKEN_USER whose SID points into the same buffer.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: `sid` is valid while `buf` lives; on success `text` is a LocalAlloc'd string.
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(|e| format!("ConvertSidToStringSidW: {e}"))?;
    // SAFETY: `text` is a valid NUL-terminated string until freed below.
    let out = unsafe { text.to_string() }.map_err(|e| e.to_string());
    // SAFETY: frees the string ConvertSidToStringSidW allocated, once.
    unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
    out
}

/// The current user's SID as text (`S-1-5-21-...`).
pub(crate) fn current_sid_string() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle is always valid; `token` receives a new handle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("OpenProcessToken: {e}"))?;
    token_sid_string(&Owned(token))
}

/// How a process relates to us.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Peer {
    /// Same session and the token's user is ours.
    SameUser,
    /// Same session; the token cannot be read (an elevated or protected
    /// process of ours).
    SameSessionUnknownUser,
    OtherUser,
    OtherSession,
}

/// Compares the process `pid` with ours: session first, then token user.
pub(crate) fn classify_process(pid: u32) -> Result<Peer, String> {
    let mut mine = 0u32;
    let mut theirs = 0u32;
    // SAFETY: plain ids and live out pointers.
    unsafe {
        ProcessIdToSessionId(GetCurrentProcessId(), &mut mine).map_err(|e| format!("ProcessIdToSessionId: {e}"))?;
        // A process whose session cannot even be read (a protected system process) is not ours.
        if ProcessIdToSessionId(pid, &mut theirs).is_err() {
            return Ok(Peer::OtherSession);
        }
    }
    if mine != theirs {
        return Ok(Peer::OtherSession);
    }
    // SAFETY: plain flag and id; the returned handle is owned below.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return Ok(Peer::SameSessionUnknownUser);
    };
    let process = Owned(process);
    let mut token = HANDLE::default();
    // SAFETY: `process` is a live handle; `token` receives a new handle.
    if unsafe { OpenProcessToken(process.get(), TOKEN_QUERY, &mut token) }.is_err() {
        return Ok(Peer::SameSessionUnknownUser);
    }
    let theirs = token_sid_string(&Owned(token))?;
    Ok(if theirs == current_sid_string()? { Peer::SameUser } else { Peer::OtherUser })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_process_is_the_same_user_in_the_same_session() {
        assert_eq!(classify_process(std::process::id()), Ok(Peer::SameUser));
    }

    #[test]
    fn a_service_session_process_is_another_session() {
        // Process 4 is the System process, always in session 0; a desktop session is never 0.
        assert_eq!(classify_process(4), Ok(Peer::OtherSession));
    }

    #[test]
    fn the_user_sid_has_the_usual_shape() {
        let sid = current_sid_string().expect("a SID");
        assert!(sid.starts_with("S-1-5-"), "{}", sid.len());
    }

    #[test]
    fn wide_strings_end_with_a_nul() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
    }
}
