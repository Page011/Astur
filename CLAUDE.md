# CLAUDE.md — Astur

Read this first, every session. Then read `AGENTS.md` and the relevant file in
`plan/` before touching code.

## Standing order: keep the docs alive

**Every prompt that changes behaviour, decisions, or traps → update `plan/` in the
same turn as the code.** Docs are not a final step; they move with the work.

- Made a design/architecture call → one line in `plan/decisions.md` (newest on
  top, dated), detail in the relevant file.
- Touched anything visual (animation, overlay, easing, flash) → `plan/animations.md`.
  **Read it before any visual change.**
- Found a slow/buggy/"don't use X because Y" Win32 call → `plan/win32-reference.md`
  + `plan/known-issues.md`, with the measurement/reason.
- Fixed a bug or resolved a doc/reality mismatch → flip the `plan/known-issues.md`
  entry to RESOLVED (don't delete history — note the supersede).
- Changed threads/state ownership/hot paths → `plan/architecture.md`.

If a change makes a doc wrong, the change is not done until the doc is right.
Honest docs are a project goal (see `AGENTS.md` "bar to hold" #5).

## What Astur is

Fast, free, portable tiling window manager for Windows 10/11. Single Rust `.exe`,
no installer, no required config. Alt-drag move, nearest-corner resize, dwindle/
master tiling, ≤10 workspaces, per-monitor status bar, composited workspace +
window animations. Left Alt is the reserved modifier (apps never see it).

## The bar to hold (don't regress these)

1. **Never break window management.** Animations/bar are cosmetics layered *over*
   a switch/tile that already happened instantly and correctly. A dropped or
   failed animation must never lose or misplace a window.
2. **Hooks are sacred.** `mouse_proc`/`keyboard_proc` are on the OS-wide input
   path. No locks without an atomic guard, no allocation, no `SetWindowPos`. Push
   a `Cmd` and let the manager/worker do the work. Measure before/after.
3. **Smooth is the headline.** Beat komorebi/GlazeWM/Seelen on motion polish.
   Silky over feature-count.
4. **Tile placement is INSTANT.** `animate_to` is a historical name — leave it
   instant. Real glide goes through the snapshot overlay, never per-frame
   real-window `SetWindowPos` (tried, removed — see `plan/known-issues.md`).
5. Keep `astur-config` (+ `layout.rs`) Win32-free / Win32-light — the trivially
   testable parts. `astur-config` is its own crate now and has zero Win32.

## Code layout (Cargo workspace, v2)

The repo is a Cargo workspace. Astur Lite = the frozen `v1.0.0` git tag (single exe,
pre-workspace); the workspace below is the evolving full "Astur" (v2). See
`plan/roadmap-v2.md`.

- `crates/astur/src/main.rs` — everything Win32: hooks, manager loop, compositors,
  bar, launcher, system menu, file search. One big translation unit by design (fast
  build, no module ceremony). The binary users run.
- `crates/astur/src/layout.rs` — pure geometry (`dwindle_layout`, `master_stack`).
  Uses the `RECT` type only; no Win32 calls.
- `crates/astur-config/src/lib.rs` — pure parsing → `Config`. **No Win32.** Shared by
  the WM and the (WIP) settings GUI so they never drift; aliased as `config` in the WM.
- `crates/astur-settings/` — the settings GUI (egui, WIP — stub for now).

NOTE: plan/ docs written before the workspace say `src/main.rs` / `src/config.rs` —
those now map to `crates/astur/src/main.rs` / `crates/astur-config/src/lib.rs`.

## Build / verify

Default toolchain is MSVC. If the MSVC linker isn't reachable in the shell
(no `link.exe` / no `vcvars`), build with the installed self-contained GNU
toolchain to verify compilation (from the workspace root builds all crates; add
`-p astur` for just the WM):

```
cargo +stable-x86_64-pc-windows-gnu build --target x86_64-pc-windows-gnu
```

If crate downloads fail with a cert-revocation error
(`CRYPT_E_NO_REVOCATION_CHECK`), prefix with `CARGO_HTTP_CHECK_REVOKE=false`.
Note: Git Bash's MSYS `link` shadows MSVC `link.exe` — don't build MSVC from Bash.

## Style

Caveman mode is on globally (terse, drop articles/filler). Code, commits, and
PRs are written normal. No emojis in code. Start answers with a clear judgment,
then the why; flag every real flaw.
