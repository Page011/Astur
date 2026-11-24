//! Opening a file or folder with the shell.

use std::path::Path;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::sys;

/// Runs `ShellExecuteExW` with the default verb on the calling thread, which
/// must be a COM STA thread (`open_path` makes one). `Err` carries the system
/// error code only: log text never holds a path.
pub(crate) fn shell_open(path: &Path) -> Result<(), i32> {
    let file = sys::wide(&path.to_string_lossy());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised and `file` outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| e.code().0)
}

/// Opens `path` on a short-lived STA worker thread: ShellExecute can wait on
/// other processes and must never run on the UI thread.
pub(crate) fn open_on_worker(path: &Path) {
    let path = path.to_path_buf();
    let spawned = std::thread::Builder::new().name("rsb-open".into()).spawn(move || {
        // SAFETY: balanced with CoUninitialize below on the same thread.
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let result = shell_open(&path);
        if com.is_ok() {
            // SAFETY: the CoInitializeEx above succeeded on this thread.
            unsafe { CoUninitialize() };
        }
        if let Err(code) = result {
            log::warn!("open failed: {code:#010x}");
        }
    });
    if spawned.is_err() {
        log::warn!("open failed: could not start a worker thread");
    }
}
