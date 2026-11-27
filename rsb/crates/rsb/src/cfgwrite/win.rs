//! The Windows calls the writer needs: the process token, what an open file
//! handle says about the file behind it, and a move that never replaces.
//! Every check is made on an open handle, never on a path, so a link swapped
//! in between cannot change what was checked.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::HSTRING;

pub(crate) const SHARE_READ: u32 = 0x1;
pub(crate) const SHARE_WRITE: u32 = 0x2;
pub(crate) const SHARE_DELETE: u32 = 0x4;
const FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const ATTR_READONLY: u32 = 0x1;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_REPARSE_POINT: u32 = 0x400;
const ACCESS_DELETE: u32 = 0x0001_0000;
const ACCESS_GENERIC_WRITE: u32 = 0x4000_0000;
const ACCESS_READ_ATTRIBUTES: u32 = 0x80;
/// The medium integrity level: a process above it is "elevated" for our purposes.
const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

/// A file's identity on its volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ident {
    pub volume: u32,
    pub index: u64,
}

/// What an open handle says about the file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    pub ident: Ident,
    pub links: u32,
    pub attrs: u32,
}

impl Facts {
    pub(crate) fn is_link(&self) -> bool {
        self.attrs & ATTR_REPARSE_POINT != 0
    }
    pub(crate) fn is_read_only(&self) -> bool {
        self.attrs & ATTR_READONLY != 0
    }
    pub(crate) fn is_dir(&self) -> bool {
        self.attrs & ATTR_DIRECTORY != 0
    }
}

fn handle_of(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

pub(crate) fn facts(file: &File) -> io::Result<Facts> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open for the life of `file`; `info` is a valid out pointer.
    unsafe { GetFileInformationByHandle(handle_of(file), &mut info) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
    Ok(Facts {
        ident: Ident {
            volume: info.dwVolumeSerialNumber,
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        },
        links: info.nNumberOfLinks,
        attrs: info.dwFileAttributes,
    })
}

/// The file behind this handle was deleted or replaced by a rename and only
/// our handle keeps it alive.
pub(crate) fn delete_pending(file: &File) -> io::Result<bool> {
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: `info` is a valid buffer of the size passed; the handle is open.
    unsafe {
        GetFileInformationByHandleEx(
            handle_of(file),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))?;
    Ok(info.DeletePending)
}

/// Opens a file to read, sharing read and delete (so our own rename over it
/// works) but not write (so an editor saving in place waits). A link is
/// opened as the link itself, and a folder can be opened to be told it is one.
pub(crate) fn open_read(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ | SHARE_DELETE)
        .custom_flags(FLAG_OPEN_REPARSE_POINT | FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Opens a file only to ask about it (identity of the file now behind a path).
pub(crate) fn open_attrs(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(ACCESS_READ_ATTRIBUTES)
        .share_mode(SHARE_READ | SHARE_WRITE | SHARE_DELETE)
        .custom_flags(FLAG_OPEN_REPARSE_POINT | FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Opens a folder and keeps it open: while it is, the folder cannot be
/// renamed or deleted, so it cannot be swapped for a link.
pub(crate) fn open_dir(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(ACCESS_READ_ATTRIBUTES)
        .share_mode(SHARE_READ | SHARE_WRITE)
        .custom_flags(FLAG_OPEN_REPARSE_POINT | FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// The lock file: no sharing, deleted when the last handle closes.
pub(crate) fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .access_mode(ACCESS_GENERIC_WRITE | ACCESS_DELETE)
        .share_mode(0)
        .custom_flags(FLAG_OPEN_REPARSE_POINT | FLAG_DELETE_ON_CLOSE)
        .open(path)
}

/// A new file that must not exist: a name that is already taken, a link
/// included, fails and is never followed.
pub(crate) fn create_new(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(SHARE_READ)
        .custom_flags(FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Moves `from` to `to`, failing when `to` exists.
pub(crate) fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let (a, b) = (HSTRING::from(from.as_os_str()), HSTRING::from(to.as_os_str()));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { MoveFileExW(&a, &b, MOVEFILE_WRITE_THROUGH) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xffff))
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcessToken and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// True when this process runs elevated or above medium integrity. A process
/// whose token cannot be read counts as elevated: nothing is written then.
fn token_is_high() -> bool {
    let mut raw = HANDLE::default();
    // SAFETY: the pseudo handle is always valid; `raw` receives a new handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) }.is_err() {
        return true;
    }
    let token = Token(raw);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: `elevation` is a valid buffer of the size passed.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    if ok.is_err() || elevation.TokenIsElevated != 0 {
        return true;
    }
    let mut needed = 0u32;
    // SAFETY: a size query; it is expected to fail with the length needed.
    let _ = unsafe { GetTokenInformation(token.0, TokenIntegrityLevel, None, 0, &mut needed) };
    if needed == 0 {
        return true;
    }
    // u64 elements so the buffer is aligned for the label's pointer field.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `needed` bytes and is suitably aligned.
    let got = unsafe {
        GetTokenInformation(token.0, TokenIntegrityLevel, Some(buf.as_mut_ptr().cast()), needed, &mut needed)
    };
    if got.is_err() {
        return true;
    }
    // SAFETY: the call above filled `buf` with a TOKEN_MANDATORY_LABEL whose SID points into the same buffer.
    let rid = unsafe {
        let label = buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>();
        let sid = (*label).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 { return true } else { *GetSidSubAuthority(sid, u32::from(count - 1)) }
    };
    rid > SECURITY_MANDATORY_MEDIUM_RID
}

#[cfg(test)]
thread_local! {
    /// A test sets this to stand in for the token.
    pub(crate) static FAKE_HIGH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Settings are not written from an elevated process: the config folder is
/// one any medium process controls, so a write from above it would be a
/// write and delete primitive.
pub(crate) fn is_elevated() -> bool {
    #[cfg(test)]
    if let Some(fake) = FAKE_HIGH.with(std::cell::Cell::get) {
        return fake;
    }
    token_is_high()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_process_runs_at_medium_integrity_or_the_machine_runs_tests_elevated() {
        // The answer is only read here; a CI box may run elevated.
        let _ = token_is_high();
    }

    #[test]
    fn a_handle_tells_the_file_and_a_second_name_for_it_is_a_link_count() {
        let dir = crate::testutil::TempDir::new("win");
        let a = dir.path().join("a.txt");
        std::fs::write(&a, b"x").unwrap();
        let f = open_read(&a).unwrap();
        let facts1 = facts(&f).unwrap();
        assert_eq!(facts1.links, 1);
        assert!(!facts1.is_link() && !facts1.is_read_only() && !facts1.is_dir());
        std::fs::hard_link(&a, dir.path().join("b.txt")).unwrap();
        assert_eq!(facts(&f).unwrap().links, 2);
        assert_eq!(facts(&open_attrs(&a).unwrap()).unwrap().ident, facts1.ident);
        let d = open_dir(dir.path()).unwrap();
        assert!(facts(&d).unwrap().is_dir());
    }

    #[test]
    fn a_rename_over_a_file_we_hold_open_leaves_the_handle_delete_pending() {
        let dir = crate::testutil::TempDir::new("win");
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        std::fs::write(&a, b"old").unwrap();
        std::fs::write(&b, b"new").unwrap();
        let held = open_read(&a).unwrap();
        assert!(!delete_pending(&held).unwrap());
        std::fs::rename(&b, &a).unwrap();
        assert!(delete_pending(&held).unwrap());
        assert_ne!(facts(&held).unwrap().ident, facts(&open_attrs(&a).unwrap()).unwrap().ident);
    }

    #[test]
    fn a_move_never_replaces_and_a_new_file_never_reuses_a_name() {
        let dir = crate::testutil::TempDir::new("win");
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        std::fs::write(&a, b"1").unwrap();
        std::fs::write(&b, b"2").unwrap();
        assert!(move_no_replace(&a, &b).is_err());
        assert_eq!(std::fs::read(&b).unwrap(), b"2");
        assert!(create_new(&b).is_err());
        std::fs::remove_file(&b).unwrap();
        move_no_replace(&a, &b).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"1");
    }

    #[test]
    fn the_lock_file_has_one_holder_and_goes_when_it_closes() {
        let dir = crate::testutil::TempDir::new("win");
        let p = dir.path().join("lock");
        let first = open_lock(&p).unwrap();
        let second = open_lock(&p).unwrap_err();
        assert_eq!(second.raw_os_error(), Some(32), "a sharing violation");
        drop(first);
        assert!(!p.exists(), "deleted on close");
        drop(open_lock(&p).unwrap());
    }
}
