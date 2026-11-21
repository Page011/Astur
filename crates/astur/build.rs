// Embed the Windows resource script (astur.rc: the app icon, the manifest and
// the version info) into astur.exe. No-op on non-Windows targets.
// The version macros come from Cargo.toml, so the exe's version cannot drift.
fn main() {
    embed_resource::compile(
        "astur.rc",
        [
            concat!("VMAJ=", env!("CARGO_PKG_VERSION_MAJOR")),
            concat!("VMIN=", env!("CARGO_PKG_VERSION_MINOR")),
            concat!("VPAT=", env!("CARGO_PKG_VERSION_PATCH")),
            concat!("VER=\"", env!("CARGO_PKG_VERSION"), "\""),
        ],
    );
}
