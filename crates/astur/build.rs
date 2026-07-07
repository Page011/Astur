// Embed the Windows resource script (the app icon) into astur.exe.
// See astur.rc / assets/astur.ico. No-op on non-Windows targets.
fn main() {
    embed_resource::compile("astur.rc", embed_resource::NONE);
}
