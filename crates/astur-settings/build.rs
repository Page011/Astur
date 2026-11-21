// Embed the Windows resource script (astur-settings.rc: the app icon, the
// manifest and the version info) into astur-settings.exe. Reuses the WM's
// assets (icon, manifest), so both exes match. The version macros come from
// Cargo.toml, so the exe's version cannot drift.
// The assets live in the sibling crate, outside Cargo's default scan, so
// list every input (listing any replaces that scan).
fn main() {
    println!("cargo:rerun-if-changed=astur-settings.rc");
    println!("cargo:rerun-if-changed=../astur/assets/astur.manifest");
    println!("cargo:rerun-if-changed=../astur/assets/astur.ico");
    embed_resource::compile(
        "astur-settings.rc",
        [
            concat!("VMAJ=", env!("CARGO_PKG_VERSION_MAJOR")),
            concat!("VMIN=", env!("CARGO_PKG_VERSION_MINOR")),
            concat!("VPAT=", env!("CARGO_PKG_VERSION_PATCH")),
            concat!("VER=\"", env!("CARGO_PKG_VERSION"), "\""),
        ],
    );
}
