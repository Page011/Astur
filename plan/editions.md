# Editions — Astur Lite vs Astur (Full)

Two products, two git branches, one repo.

| | **Astur Lite** | **Astur** (Full) |
|---|---|---|
| Git branch | `lite` (from `cc7e441` / v1.0.0) | `main` (v2 workspace) |
| Distribution | single portable `.exe`, no install | installer (winget/MSI) |
| Process | **console window** (Ctrl+C to quit) | **tray icon**, no console |
| Stop / settings | Ctrl+C in console; hand-edit `.conf` | tray icon menu: Settings / Quit |
| RAM | ~1 MB, minimal | higher (GUI, file index) |
| Config | hand-edit `astur.conf` | **settings GUI** + `.conf` |
| Tiling / Alt-drag / resize | yes | yes |
| Per-monitor bar / workspaces | yes | yes |
| Workspace + window animations | yes | yes |
| App launcher (Alt+Space) | — | yes (+ icons) |
| File search | — | yes (Windows Search; MFT later) |
| System / power menu (Alt+Shift+Space) | — | yes (categorised) |
| Settings GUI | — | yes (`astur-settings`, egui) |
| Tray icon | — | yes |
| Auto-start on login | manual | installer option |
| Update cadence | small **efficiency/quality** only | features **+** efficiency |

**Positioning:** Lite = the lean keyboard/console WM for power users who want ~1MB and
text config. Astur = the friendly, installable app (tray + GUI + launcher + search) for
everyone else. Same motion polish + core tiling in both.

## Branch model

- `lite` — the minimal single-crate app (console). Gets **small, core-only** updates:
  efficiency (binary size, fewer allocs, hook latency) and quality (bug fixes). **Never**
  the big features (launcher/search/sysmenu/GUI). Tag releases `v1.0.x`.
- `main` — the full workspace app (`crates/astur` + `astur-config` + `astur-settings`).
  Features + efficiency. Tag releases `v2.x`.

### The backport reality (the cost of maintaining both)

A fix to the **shared WM core** (hooks, manager, tiling, bar, animations, drag) that you
want in BOTH must be applied to each branch — they have different file layouts
(`src/main.rs` on `lite`; `crates/astur/src/main.rs` on `main`), so it's a manual port,
not a clean cherry-pick. This is infrequent (Lite is stable) but real. Examples that
*should* go to both: the workspace-switch flash fix, lockless `PRESSED`, the
`switch_plain` Vec-clone removal.

**Long-term option (not now):** extract the shared WM core into a crate both editions
consume, with Full's extras behind a `full` cargo feature — then Lite is
`--no-default-features` of the same code and backports vanish. Big refactor; revisit if
the backport tax becomes annoying.

## Lite's first maintained release — v1.0.1 (proposed)

Pure efficiency/quality, no features — exactly Lite's remit. Port from `main`'s v2 work:
1. **Workspace-switch flash fix** (show overlay before presenting frame 0) — a real bug.
2. **Lockless `PRESSED`** bitset (drop a Mutex from the keyboard hot path).
3. **`switch_plain`** no longer clones the window Vec per switch.
Then tag `v1.0.1`, build the portable exe. Marketing: "Astur Lite 1.0.1 — smoother,
leaner."

## Full's distinguishing work (on `main`)

1. **Tray icon + no console** — SHIPPED 2026-06-28. `#![cfg_attr(not(debug_assertions),
   windows_subsystem = "windows")]` drops the console in **release** (debug keeps it for
   dev). `setup_tray` adds a `Shell_NotifyIcon`: left/double-click → `tray_open_settings`
   (launches the sibling `astur-settings.exe`); right-click → popup menu Settings / Quit;
   **Quit = `tray_remove` + `restore_all_windows` + `PostQuitMessage`** (the only exit
   path now there's no console). Still TODO: embed a **custom `.ico`** (currently the
   generic `IDI_APPLICATION` — needs an icon asset + a build script, e.g. `winres`).
2. **Settings GUI** (`astur-settings`, egui) editing `astur.conf` via `astur-config`
   (needs a `save_config(&Config) -> String` writer). Separate process.
3. **Installer** (winget first, then MSI/Inno): bundle WM exe + `astur-settings.exe` +
   Start-Menu shortcut + optional autostart + uninstaller. Keep the portable exe too.

## README / website messaging (must match this)

- **`main` README**: describe Astur (full) — tiling + launcher + file search + system
  menu + GUI + tray + installer. Add an **Editions** section + the table above, linking
  the Lite download/branch. (Current `main` README still describes only the v1.0.0 core —
  needs this update.)
- **`lite` README** (on the `lite` branch): describe Astur Lite — minimal, portable,
  console, ~1MB — and link "want a GUI / launcher / search? → Astur (full)".
- **Website (astur.app)**: a clear "Lite vs Full" comparison (this table) + two download
  buttons (Lite portable `.exe`, Astur installer). I can't edit the site — mirror this
  messaging there.
