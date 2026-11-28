//! One bar per user session: a named mutex in the Local namespace.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::HSTRING;

pub const MUTEX_NAME: &str = "Local\\RSB.Bar";

/// Held for the life of the bar; released on drop.
pub struct InstanceGuard {
    handle: HANDLE,
}

pub enum Instance {
    First(InstanceGuard),
    AlreadyRunning,
}

pub fn acquire() -> Result<Instance, String> {
    match crate::pipe::test_tag() {
        Some(tag) => acquire_named(&format!("{MUTEX_NAME}.{tag}")),
        None => acquire_named(MUTEX_NAME),
    }
}

/// `CreateMutexW` plus `ERROR_ALREADY_EXISTS` decides. A mutex that cannot be
/// created at all is an error, never treated as "first".
pub fn acquire_named(name: &str) -> Result<Instance, String> {
    let wide = HSTRING::from(name);
    // SAFETY: `wide` outlives the call; default security, not initially owned
    // (the handle's existence is the lock; nobody ever waits on it).
    let handle = unsafe { CreateMutexW(None, false, &wide) }.map_err(|e| format!("CreateMutexW: {e}"))?;
    // SAFETY: read straight after the call that sets it, on the same thread.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if existed {
        // SAFETY: the handle was just returned to us and is not used again.
        let _ = unsafe { CloseHandle(handle) };
        Ok(Instance::AlreadyRunning)
    } else {
        Ok(Instance::First(InstanceGuard { handle }))
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: closes the handle made in `acquire_named`.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique(tag: &str) -> String {
        format!("Local\\RSB.Test.{}.{tag}", std::process::id())
    }

    #[test]
    fn a_second_acquire_sees_the_first_and_release_frees_it() {
        let name = unique("single");
        let Ok(Instance::First(guard)) = acquire_named(&name) else { panic!("the first caller wins") };
        for _ in 0..3 {
            assert!(matches!(acquire_named(&name), Ok(Instance::AlreadyRunning)));
        }
        drop(guard);
        assert!(matches!(acquire_named(&name), Ok(Instance::First(_))), "free after the guard drops");
    }

    #[test]
    fn different_names_do_not_collide_and_the_real_one_is_per_session() {
        assert!(matches!(acquire_named(&unique("a")), Ok(Instance::First(_))));
        assert!(matches!(acquire_named(&unique("b")), Ok(Instance::First(_))));
        assert!(MUTEX_NAME.starts_with("Local\\"));
    }
}
