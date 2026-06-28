// Astur settings GUI — WIP placeholder.
//
// Plan (plan/roadmap-v2.md): an egui (eframe) app that reads/writes astur.conf via
// the shared `astur-config` crate, so the WM and the GUI never drift. It runs as a
// SEPARATE process from the window manager — a GUI crash must never touch the input
// hooks or the manager ("never break window management"). Launched from the system
// menu (Setup) and/or a tray icon.
//
// This stub exists so the workspace compiles and the crate slot is reserved. The real
// egui UI replaces it next.

fn main() {
    println!("astur-settings (WIP) — friendly editor for astur.conf.");
    println!("Planned egui app over the shared astur-config crate. See plan/roadmap-v2.md.");
}
