//! Where the config folder and the log folder are.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

/// The Roaming or Local application data folder, from the shell, falling
/// back to the environment variable when the shell call fails.
fn known_folder(roaming: bool) -> Option<PathBuf> {
    let id = if roaming { &FOLDERID_RoamingAppData } else { &FOLDERID_LocalAppData };
    // SAFETY: `id` is a valid folder id; on success the result is a COM
    // allocation that is freed below after copying it out.
    let path = unsafe {
        let text = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast()));
        s
    };
    path.map(PathBuf::from)
        .or_else(|| std::env::var_os(if roaming { "APPDATA" } else { "LOCALAPPDATA" }).map(PathBuf::from))
}

fn absolute(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

/// `--config-dir`, else `RSB_CONFIG_DIR`, else `%APPDATA%\rsb`.
pub fn config_dir(arg: Option<&Path>) -> PathBuf {
    resolve_config(arg, std::env::var_os("RSB_CONFIG_DIR").map(PathBuf::from), || known_folder(true))
}

/// `--log-dir`, else `%LOCALAPPDATA%\rsb\logs`.
pub fn log_dir(arg: Option<&Path>) -> PathBuf {
    resolve_log(arg, || known_folder(false))
}

fn resolve_config(arg: Option<&Path>, env: Option<PathBuf>, roaming: impl FnOnce() -> Option<PathBuf>) -> PathBuf {
    if let Some(a) = arg {
        return absolute(a.to_path_buf());
    }
    if let Some(e) = env.filter(|e| !e.as_os_str().is_empty()) {
        return absolute(e);
    }
    absolute(roaming().unwrap_or_else(|| PathBuf::from(".")).join("rsb"))
}

fn resolve_log(arg: Option<&Path>, local: impl FnOnce() -> Option<PathBuf>) -> PathBuf {
    if let Some(a) = arg {
        return absolute(a.to_path_buf());
    }
    absolute(local().unwrap_or_else(|| PathBuf::from(".")).join("rsb").join("logs"))
}

/// The environment widgets' `~` and `%APPDATA%` paths expand against.
pub fn path_env() -> rsb_core::permissions::PathEnv {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from).unwrap_or_default();
    rsb_core::permissions::PathEnv {
        home: var("USERPROFILE"),
        appdata: known_folder(true).unwrap_or_else(|| var("APPDATA")),
        local_appdata: known_folder(false).unwrap_or_else(|| var("LOCALAPPDATA")),
        temp: var("TEMP"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argument_beats_the_environment_beats_appdata() {
        let appdata = || Some(PathBuf::from(r"C:\Users\x\AppData\Roaming"));
        assert_eq!(resolve_config(Some(Path::new(r"C:\a")), Some(r"C:\b".into()), appdata), PathBuf::from(r"C:\a"));
        assert_eq!(resolve_config(None, Some(r"C:\b".into()), appdata), PathBuf::from(r"C:\b"));
        assert_eq!(resolve_config(None, Some("".into()), appdata), PathBuf::from(r"C:\Users\x\AppData\Roaming\rsb"));
        assert_eq!(resolve_config(None, None, appdata), PathBuf::from(r"C:\Users\x\AppData\Roaming\rsb"));
    }

    #[test]
    fn logs_default_under_local_appdata() {
        let local = || Some(PathBuf::from(r"C:\Users\x\AppData\Local"));
        assert_eq!(resolve_log(None, local), PathBuf::from(r"C:\Users\x\AppData\Local\rsb\logs"));
        assert_eq!(resolve_log(Some(Path::new(r"D:\logs")), local), PathBuf::from(r"D:\logs"));
    }

    #[test]
    fn relative_arguments_become_absolute() {
        assert!(config_dir(Some(Path::new("cfg"))).is_absolute());
        assert!(log_dir(Some(Path::new("logs"))).is_absolute());
    }

    #[test]
    fn the_shell_knows_both_folders() {
        assert!(known_folder(true).is_some_and(|p| p.is_absolute()));
        assert!(known_folder(false).is_some_and(|p| p.is_absolute()));
    }
}
