//! Opening a file a widget may read. Shared by `rsb.file.read`,
//! `rsb.file.watch` and the binary's image loader, so every file a widget
//! reaches goes through the same checks.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use rsb_core::io::{IoError, IoGrant, limits};
use rsb_core::permissions::PathEnv;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_NAME_NORMALIZED, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};

const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Opens `path` for reading after the lexical check
/// (`permissions::check_path_text`) and the grant check, then reads the
/// opened handle's final path (`GetFinalPathNameByHandleW`, normalised,
/// compared without case) and checks that against the grant again, so a
/// junction or link inside a permitted folder cannot lead outside it. The
/// caller reads from the returned handle, never by path a second time.
/// A sharing violation (an editor still writing) is retried
/// `limits::FILE_SHARING_RETRIES` times, `FILE_SHARING_RETRY_DELAY` apart.
/// Errors never hold the path.
pub fn open_permitted(path: &Path, grant: &IoGrant) -> Result<File, IoError> {
    if let Err(need) = grant.permissions.check_file(path, &grant.widget_dir, &grant.env) {
        let _ = need;
        return Err(IoError::NotPermitted(format!("file:{}", short_path(path, &grant.env))));
    }
    let file = open_with_retries(path)?;
    let meta = file.metadata().map_err(|e| io_error(&e))?;
    if !meta.is_file() {
        return Err(IoError::Io("not a regular file".into()));
    }
    let final_path = final_path(&file).ok_or_else(|| IoError::Io("could not resolve the file".into()))?;
    if !final_path_permitted(&final_path, grant) {
        return Err(IoError::NotPermitted(format!(
            "file:{} (it resolves outside the permitted places)",
            short_path(path, &grant.env)
        )));
    }
    Ok(file)
}

/// Reads at most `max_bytes` from an opened file as lossy UTF-8; a longer
/// file is `TooLarge`.
pub fn read_text(mut file: File, max_bytes: usize) -> Result<String, IoError> {
    let max = max_bytes.min(limits::FILE_MAX_BYTES);
    if file.metadata().map_or(0, |m| m.len()) > max as u64 {
        return Err(IoError::TooLarge { limit: max });
    }
    let mut bytes = Vec::new();
    (&mut file).take(max as u64 + 1).read_to_end(&mut bytes).map_err(|e| io_error(&e))?;
    if bytes.len() > max {
        return Err(IoError::TooLarge { limit: max });
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn open_with_retries(path: &Path) -> Result<File, IoError> {
    let mut tries = 0;
    loop {
        match OpenOptions::new().read(true).share_mode(7).open(path) {
            Ok(f) => return Ok(f),
            Err(e) if is_sharing_violation(&e) && tries < limits::FILE_SHARING_RETRIES => {
                tries += 1;
                std::thread::sleep(limits::FILE_SHARING_RETRY_DELAY);
            }
            Err(e) => return Err(io_error(&e)),
        }
    }
}

fn is_sharing_violation(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION))
}

/// Maps an OS error to ours without the path or the OS's own text.
pub(crate) fn io_error(e: &std::io::Error) -> IoError {
    if is_sharing_violation(e) {
        return IoError::Io("the file is in use by another program".into());
    }
    match e.kind() {
        std::io::ErrorKind::NotFound => IoError::NotFound,
        std::io::ErrorKind::PermissionDenied => IoError::Denied,
        kind => IoError::Io(format!("could not read the file ({kind})")),
    }
}

/// The handle's final path without the `\\?\` prefix; None for a network
/// path or on failure.
fn final_path(file: &File) -> Option<PathBuf> {
    let handle = HANDLE(file.as_raw_handle());
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: `handle` is the live handle of `file`; `buf` is a writable slice.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                &mut buf,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
            )
        } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            break;
        }
        buf.resize(n + 1, 0);
    }
    let text = String::from_utf16_lossy(&buf);
    if text.starts_with("\\\\?\\UNC\\") {
        return None;
    }
    Some(PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text).to_string()))
}

/// Without the `\\?\` prefix `canonicalize` adds.
fn canonical(p: &Path) -> PathBuf {
    match std::fs::canonicalize(p) {
        Ok(c) => {
            let t = c.to_string_lossy();
            PathBuf::from(t.strip_prefix("\\\\?\\").unwrap_or(&t).to_string())
        }
        Err(_) => p.to_path_buf(),
    }
}

fn final_path_permitted(final_path: &Path, grant: &IoGrant) -> bool {
    // The grant's own folders may themselves be reached through a link, so
    // compare against their resolved form as well as the written one.
    let resolved_env = PathEnv {
        home: canonical(&grant.env.home),
        appdata: canonical(&grant.env.appdata),
        local_appdata: canonical(&grant.env.local_appdata),
        temp: canonical(&grant.env.temp),
    };
    let resolved_dir = canonical(&grant.widget_dir);
    grant.permissions.check_file(final_path, &resolved_dir, &resolved_env).is_ok()
        || grant.permissions.check_file(final_path, &grant.widget_dir, &grant.env).is_ok()
}

/// A path as the plate and log may show it: under a known folder as
/// `~/...`, `%APPDATA%/...`; anywhere else only the file name.
pub(crate) fn short_path(path: &Path, env: &PathEnv) -> String {
    let lower = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
    let full = lower(path);
    for (prefix, label) in
        [(&env.temp, "%TEMP%"), (&env.local_appdata, "%LOCALAPPDATA%"), (&env.appdata, "%APPDATA%"), (&env.home, "~")]
    {
        let base = lower(prefix);
        let base = base.trim_end_matches('\\');
        if !base.is_empty() && full.len() > base.len() && full.starts_with(base) && full.as_bytes()[base.len()] == b'\\'
        {
            let original = path.to_string_lossy().replace('\\', "/");
            let rest = original.get(base.len()..).unwrap_or("").trim_start_matches('/');
            return format!("{label}/{rest}");
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("<elsewhere>/{name}")
}
