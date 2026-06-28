# plan/ — design notes, discussions, decisions

Working memory for Astur. Anything worth remembering that isn't code: design
decisions, why something was done (or removed), traps, competitor notes, ideas.

Keep entries dated. When a decision changes, append — don't silently rewrite
history (note the supersede).

## Index

| File | Purpose |
|---|---|
| `architecture.md` | Component map, threads, data flow, state ownership. |
| `animations.md` | Animation design. Desktop-switch modes (off/slide/spring/fade) + honest window-animation analysis. **Read before touching anything visual.** |
| `launcher.md` | App picker (Alt+Space): v1/v2 as built + Phase 3 file-search plan. |
| `optimization.md` | Phase 4 optimization pass plan (latency/anim/size/RAM/startup). |
| `mods.md` | Extensibility design: declarative mods + out-of-process IPC mods + security. |
| `system-menu.md` | Alt+Shift+Space power menu (shipped) + wallpaper/category backlog. |
| `roadmap-v2.md` | Strategy: GUI config (egui), installer, v1/v2 framing, MFT search, Tab columns. |
| `editions.md` | Astur Lite (`lite` branch) vs Astur Full (`main`): feature matrix, branch/backport model, tray. |
| `win32-reference.md` | Win32 APIs in use, official doc links, known-slow / known-buggy calls. |
| `competitors.md` | komorebi, GlazeWM, Seelen UI, FancyZones — features + links. |
| `known-issues.md` | Bugs, doc/reality mismatches, code to use / avoid. |
| `ideas.md` | Backlog and open questions. |
| `decisions.md` | One-line ADR log: what we decided and when. |

## How to use this dir

- New investigation / chat that produced a conclusion → drop a dated section in
  the most relevant file (or a new file).
- Found a Win32 call that's slow/buggy → `win32-reference.md` + `known-issues.md`.
- Made an architectural call → one line in `decisions.md`, detail elsewhere.
- "Don't use X because Y" → `known-issues.md`, with the measurement/reason.
