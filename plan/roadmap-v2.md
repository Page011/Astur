# Roadmap — v2 direction (GUI config, installer, packaging)

Judgment-first answers to the 2026-06-27 strategy questions, then the plan. Written
to be critical (per the project bar), not cheerleading.

## TL;DR recommendations

1. **GUI config: YES** — but as a **separate companion app** that edits the *same
   config file*. Keep the text config as the source of truth (power users + hot-reload
   + mods all depend on it). Don't link the GUI into the WM process.
2. **Toolkit: egui, not iced** (for a settings panel). Lighter, immediate-mode, far
   faster to build forms. iced's Elm architecture is overkill for a settings screen.
3. **Installer: YES as an *option*, not a replacement.** Keep the portable single-exe.
   The installer just packages exe + GUI + autostart for non-technical users.
4. **v1/v2 split: REFRAME.** Don't fork two codebases — that's a maintenance trap.
   One core WM + **optional companions**. "Minimal" = exe only (~1MB). "Full" = exe +
   settings GUI + installer. Same core.
5. **"Instant like Everything" file search: needs the MFT/USN in-RAM index** (v2,
   admin + ~tens-MB RAM). The shipped CONTAINS switch (~100ms) is the no-admin middle
   ground; it is NOT literally instant and is word-prefix, not substring.

## 1. GUI config (replace hand-editing `.conf`)

**Why yes:** the `.conf` barrier is real — komorebi (JSON/YAML + `komorebic` CLI) and
GlazeWM (YAML) both lose non-technical users here. A friendly settings GUI is the
single highest-value adoption move and a real differentiator.

**How (critical constraints):**
- **Separate process / binary** (`astur-settings.exe`), NOT linked into the WM. A GUI
  crash or a heavy egui repaint must never touch the input hooks or the manager
  ("never break window management"). Same isolation logic as out-of-process mods.
- **The config file stays the source of truth.** The GUI is a *friendly editor* over
  `astur.conf`/`navbar.conf`: read on open, write on change. The WM already
  hot-reloads on save (`config_watcher`), so the GUI changing the file applies live —
  no new plumbing in the WM. Power users keep text config; both edit the same file.
- Launch the GUI from the **system menu → Setup** (categorised menu, below) and/or a
  tray icon. The WM spawns it with `ShellExecute`.
- egui via `eframe`. Group settings into tabs mirroring the menu categories
  (Tiling, Appearance/Theme, Bar, Launcher, Keybinds, Rules, Mods). Live preview where
  cheap (colours).

**Risks/flaws to watch:** config round-trips must preserve comments + unknown keys
(don't clobber a power user's hand-tuned file). Safer: the GUI parses to the `Config`
model, but writes by *updating known keys in place* and appending new ones, preserving
the documented template comments. Or: keep a canonical writer in `config.rs` that
re-emits the fully-commented file from `Config` (we already generate the documented
default template — reuse that path so the GUI's output is identical in spirit).

## 2. Installer vs single-exe

Keep BOTH. The portable exe (no admin, USB-friendly) is a genuine strength — don't
discard it. Add an installer as a *distribution option*:
- Package: the WM exe + `astur-settings.exe` + a Start-Menu shortcut + optional
  "start on login" (a `Run` registry entry or a Startup-folder shortcut) + uninstaller.
- Tooling: **winget** manifest (easy, modern, what devs expect) and/or a small
  **NSIS/Inno Setup/WiX MSI**. winget first (lowest effort, good reach).
- The installer installs the *same* portable exe — no code changes to the WM. It's a
  packaging job, not an architecture change.

## 3. v1 / v2 positioning — reframe to avoid a fork

Two literal versions = version drift, double bug-fixing, user confusion. Instead:
- **One core WM codebase.**
- **Optional companions** built from the same repo: the settings GUI, the installer,
  the (future) MFT index, downloadable mods.
- Marketing tiers map to *bundles*, not branches:
  - **Astur Lite** = the exe only (~1MB, no GUI, text config). The current ethos.
  - **Astur** (the "v2" experience) = exe + settings GUI + installer + autostart, for
    non-technical users.
- Feature-gate the heavier bits (MFT index, GUI) so Lite stays tiny. Same `main.rs`,
  cargo features / separate companion crates in a workspace.

**Restructure when this lands:** turn the repo into a **Cargo workspace**:
`astur` (WM core, today's `main.rs`), `astur-settings` (egui GUI), maybe
`astur-index` (MFT/USN crate, feature-gated), shared `astur-config` crate (move
`config.rs` here so both the WM and the GUI parse the exact same model — single
source of truth, still Win32-free + testable).

## 4. "Instant like Everything" file search — the honest path

The user's bar is literal Everything speed (<10ms, pure substring). Windows Search
can't do that: leading-wildcard `LIKE` scans (~900ms); `CONTAINS` (~100ms, shipped) is
fast-ish but word-prefix, not substring. To actually match Everything:

- **Build the MFT/USN in-RAM index** (Everything's approach): read each NTFS volume's
  Master File Table once into an in-memory filename list, then subscribe to the USN
  change journal to keep it live. Substring search is then a RAM scan over ~hundreds of
  thousands of names — **single-digit ms**, true substring, sortable columns trivial.
- **Cost:** needs **admin** (volume read handle `\\.\C:`), per NTFS volume, ~tens of MB
  RAM for the name table, real code (MFT parse + USN replay + persistence). This is why
  it's a v2 / Lite-excluded feature — it breaks the "minimal RAM, no admin" Lite ethos.
- **Plan:** a feature-gated `astur-index` module/crate. Launcher gains a provider that
  prefers the MFT index when present (admin), else falls back to CONTAINS (no admin).
  Sortable columns (below) are driven by the in-RAM records (name/path/size/modified).
- Prototype the MFT parse + USN subscribe in a scratchpad probe first (as with the
  OLE DB path) before any integration.

## 5. Tab → sortable columns (Everything-style) — launcher plan

Make Tab switch the file results into a **table view** with columns Name | Path |
Size | Modified, with **clickable/sortable** ordering:
- State: `detail: bool` (already exists, becomes "table mode") + `sort_col: enum`,
  `sort_desc: bool`.
- Render: a header row (column titles, current sort marked with ▲/▼) + rows laid out in
  fixed column rects; right-align Size/Modified; truncate Name/Path with ellipsis.
- Sort: Tab cycles, or dedicated keys (e.g. F2/F3 or Ctrl+1..4) cycle the sort column;
  re-press flips asc/desc. Sort happens on `st.files` in memory (it's already a Vec).
  When backed by the MFT index, sort over the full result set is instant.
- Keep the compact single-line list as the default (apps + files); Tab expands to the
  table for the file results. (With the OLE DB backend, results are capped at 40; the
  MFT backend can show thousands — that's where sortable columns really shine.)

This is a meaningful GDI render change but bounded; can land before the GUI. It's also
a strong candidate to live in the v2 experience.

## Suggested order

1. (done) CONTAINS speed fix.
2. (done) System-menu categories.
3. (done 2026-06-28) Workspace restructure — `crates/astur` (WM),
   `crates/astur-config` (shared, Win32-free), `crates/astur-settings` (egui stub).
   Astur Lite stays the frozen `v1.0.0` tag.
4. (done 2026-07-13) **Settings GUI shipped** — eframe/egui 0.31 app: sidebar
   sections (General / Layout / Focus / Animations / Appearance / Bar / Widgets /
   Hotkeys / Rules / About), edits BOTH confs via `astur-config`'s
   comment-preserving `set_conf_key`/`apply_updates` writer (replaces the value in
   place, collapses duplicate keys, appends missing ones), WM hot-reload applies
   live. Launched from the tray AND sysmenu Setup → Settings. Dirty-tracking via
   `Config: PartialEq` + text mirrors; invalid key names flagged inline and dropped
   on save (never writes what the parser wouldn't accept).
5. (done 2026-07-08, Inno Setup) installer packaging; winget manifest still open.
6. Tab wide columns shipped 2026-07-10; sortable ordering still open (below).
7. MFT index (feature-gated `astur-index` crate) for true instant + big result sets.

## Feature queue (user-picked 2026-07-13, in rough order)

All 16 proposed features were accepted. Shipped in the 2026-07-13 pass: inline
calculator, web-search fallback, volume widget, network widget, app buttons,
wheel-cycles-workspaces, floating rounded bar, auto-hide bar, configurable widget
zones, theme dark/light/auto, acrylic (experimental), settings GUI. Still queued:

1. **Alt+Tab replacement** — workspace-aware switcher, live DWM thumbnails
   (`DwmRegisterThumbnail` per candidate into a launcher-styled popup), hook eats
   Alt+Tab while enabled (config-gated; keep the pass-through default until solid).
2. **Clipboard history** — `AddClipboardFormatListener` on a message-only window,
   ring buffer of recent text entries, launcher prefix (e.g. `;`) lists + Enter
   re-copies/pastes.
3. **Scratchpad terminal** — Alt+` drops the configured terminal from the top edge
   (adopt-by-class, park off-screen when dismissed — same parking trick as drags).
4. **Emoji picker** — `:` prefix in the launcher over a curated static table.
5. **Per-workspace wallpapers** — `IDesktopWallpaper` COM on switch; wallpaper_dir
   config + sysmenu Theme category.
6. **Media / now-playing widget** — needs WinRT
   (`GlobalSystemMediaTransportControlsSessionManager`); weigh the `windows`
   WinRT feature cost before committing.
