//! Embeds every file under `assets/` (built-in widgets, presets, themes and
//! first-run templates) so the binary needs nothing next to it, and compiles
//! rsb.rc (icon and version info) into rsb.exe.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What `canonicalize` puts in front of a Windows path.
const VERBATIM_PREFIX: &str = "\\\\?\\";

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push((rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"), path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"));
    let assets = manifest.join("..").join("..").join("assets");
    println!("cargo:rerun-if-changed={}", assets.display());
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    walk(&assets, &assets, &mut files);
    let mut code = String::from("pub static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
        // `{:?}` escapes backslashes, so a Windows path is a valid literal.
        let _ = writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?})),",
            abs.to_string_lossy().trim_start_matches(VERBATIM_PREFIX)
        );
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets the out dir")).join("assets.rs");
    std::fs::write(out, code).expect("the generated file can be written");

    // rsb.rc (the icon and the version info) into rsb.exe, the way Astur does
    // it. The version macros come from Cargo.toml, so the exe's version cannot
    // drift. No-op on non-Windows targets.
    println!("cargo:rerun-if-changed=rsb.rc");
    println!("cargo:rerun-if-changed=rsb.ico");
    embed_resource::compile(
        "rsb.rc",
        [
            concat!("VMAJ=", env!("CARGO_PKG_VERSION_MAJOR")),
            concat!("VMIN=", env!("CARGO_PKG_VERSION_MINOR")),
            concat!("VPAT=", env!("CARGO_PKG_VERSION_PATCH")),
            concat!("VER=\"", env!("CARGO_PKG_VERSION"), "\""),
        ],
    );
}
