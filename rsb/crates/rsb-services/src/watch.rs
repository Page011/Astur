//! Folder watching with `ReadDirectoryChangesW` (overlapped, on its own
//! thread). Used for the config folder by the binary and for
//! `rsb.file.watch` by the file service.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_MODIFIED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME,
    FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{INFINITE, ResetEvent, WaitForMultipleObjects};
use windows::core::PCWSTR;

use crate::win::{Owned, manual_event, signal, wide};

/// Large enough for a burst of saves; on overflow the kernel reports zero
/// bytes and we say "everything changed".
const BUFFER_BYTES: usize = 64 * 1024;

pub struct DirWatcher {
    stop: Option<std::sync::Arc<Owned>>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl DirWatcher {
    /// Watches `dir` (and below when `recursive`). Changes are collected for
    /// `debounce` after the first one, then `sink` gets the changed paths
    /// (absolute, deduplicated) on the watcher thread. A buffer overflow is
    /// reported as the folder itself, meaning "re-read everything".
    pub fn start(
        dir: &Path,
        recursive: bool,
        debounce: Duration,
        sink: Box<dyn FnMut(Vec<PathBuf>) + Send>,
    ) -> std::io::Result<DirWatcher> {
        let wide_dir = wide(&dir.to_string_lossy());
        // SAFETY: `wide_dir` is NUL-terminated and outlives the call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide_dir.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        let handle = Owned(handle);
        let stop = std::sync::Arc::new(manual_event().map_err(std::io::Error::other)?);
        let io_event = manual_event().map_err(std::io::Error::other)?;
        let (done_tx, done) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread_stop = stop.clone();
        let root = dir.to_path_buf();
        let thread = std::thread::Builder::new().name("rsb-dirwatch".into()).spawn(move || {
            watch_loop(&root, handle, io_event, &thread_stop, recursive, debounce, sink, &ready_tx);
            let _ = done_tx.send(());
        })?;
        // A change made right after `start` returns must not slip in before the first read is issued.
        let _ = ready_rx.recv_timeout(Duration::from_secs(1));
        Ok(DirWatcher { stop: Some(stop), thread: Some(thread), done })
    }
}

impl Drop for DirWatcher {
    /// Cancels the read and joins the thread (bounded to 1 s).
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            signal(&stop);
        }
        if self.done.recv_timeout(Duration::from_secs(1)).is_ok()
            && let Some(t) = self.thread.take()
        {
            let _ = t.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn watch_loop(
    root: &Path,
    dir: Owned,
    io_event: Owned,
    stop: &Owned,
    recursive: bool,
    debounce: Duration,
    mut sink: Box<dyn FnMut(Vec<PathBuf>) + Send>,
    ready: &mpsc::Sender<()>,
) {
    // u64 elements: FILE_NOTIFY_INFORMATION needs 4-byte alignment.
    let mut buffer = vec![0u64; BUFFER_BYTES / 8];
    let mut overlapped = OVERLAPPED { hEvent: io_event.get(), ..Default::default() };
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_CREATION;
    let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
    let mut flush_at: Option<Instant> = None;
    let mut issued = false;
    loop {
        if !issued {
            // SAFETY: `buffer` and `overlapped` stay alive and unmoved until the read completes or is cancelled below.
            let r = unsafe {
                ReadDirectoryChangesW(
                    dir.get(),
                    buffer.as_mut_ptr().cast(),
                    BUFFER_BYTES as u32,
                    recursive,
                    filter,
                    None,
                    Some(&mut overlapped),
                    None,
                )
            };
            if let Err(e) = r {
                log::warn!("watching a folder failed: {e}");
                break;
            }
            issued = true;
            let _ = ready.send(());
        }
        let timeout = match flush_at {
            Some(t) => t.saturating_duration_since(Instant::now()).as_millis().min(u128::from(u32::MAX - 1)) as u32,
            None => INFINITE,
        };
        crate::count_source_wake();
        // SAFETY: both handles are live events owned by this function's caller or by `stop`'s Arc.
        let waited = unsafe { WaitForMultipleObjects(&[io_event.get(), stop.get()], false, timeout) };
        if waited == WAIT_TIMEOUT {
            if !pending.is_empty() {
                sink(std::mem::take(&mut pending).into_iter().collect());
            }
            flush_at = None;
            continue;
        }
        if waited != WAIT_OBJECT_0 {
            // Stop requested (or the wait failed): cancel the read and wait for it to finish.
            break;
        }
        let mut bytes = 0u32;
        // SAFETY: the overlapped read above completed (its event is signalled).
        let got = unsafe { GetOverlappedResult(dir.get(), &overlapped, &mut bytes, false) };
        issued = false;
        // SAFETY: resets the event so the next read can signal it again.
        let _ = unsafe { ResetEvent(io_event.get()) };
        match got {
            Ok(()) if bytes == 0 => {
                pending.insert(root.to_path_buf());
            }
            Ok(()) => collect(root, &buffer, bytes as usize, &mut pending),
            Err(e) => {
                log::warn!("watching a folder failed: {e}");
                break;
            }
        }
        if flush_at.is_none() && !pending.is_empty() {
            flush_at = Some(Instant::now() + debounce);
        }
    }
    if issued {
        // SAFETY: cancels our one pending read, then waits for the kernel to finish with the buffer.
        unsafe {
            let _ = CancelIoEx(dir.get(), Some(&overlapped));
            let mut bytes = 0u32;
            let _ = GetOverlappedResult(dir.get(), &overlapped, &mut bytes, true);
        }
    }
}

fn collect(root: &Path, buffer: &[u64], bytes: usize, out: &mut BTreeSet<PathBuf>) {
    let base = buffer.as_ptr().cast::<u8>();
    let mut offset = 0usize;
    loop {
        if offset + size_of::<FILE_NOTIFY_INFORMATION>() > bytes {
            break;
        }
        // SAFETY: the kernel wrote a chain of FILE_NOTIFY_INFORMATION records inside the first `bytes` bytes;
        // each starts 4-byte aligned and its name lies within the record (checked against `bytes` below).
        let info = unsafe { &*base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
        let name_bytes = info.FileNameLength as usize;
        let name_start = offset + std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
        if name_start + name_bytes > bytes {
            break;
        }
        let action = info.Action;
        if action == FILE_ACTION_ADDED
            || action == FILE_ACTION_REMOVED
            || action == FILE_ACTION_MODIFIED
            || action == FILE_ACTION_RENAMED_OLD_NAME
            || action == FILE_ACTION_RENAMED_NEW_NAME
        {
            // SAFETY: `name_bytes` bytes of UTF-16 follow `FileName`, inside the buffer (checked above).
            let name = unsafe { std::slice::from_raw_parts(base.add(name_start).cast::<u16>(), name_bytes / 2) };
            out.insert(root.join(OsString::from_wide(name)));
        }
        if info.NextEntryOffset == 0 {
            break;
        }
        offset += info.NextEntryOffset as usize;
    }
}
