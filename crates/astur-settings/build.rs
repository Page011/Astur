// Embed the Windows resource script (the app icon) into astur-settings.exe.
// Reuses the WM's assets/astur.ico so both exes carry the hawk.
fn main() {
    embed_resource::compile("astur-settings.rc", embed_resource::NONE);
}
