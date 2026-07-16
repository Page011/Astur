# Mods / extensibility — design notes (NOT built yet)

User goal: let people **customise their own stuff** and let us **publish downloadable
"mods"** that extend Astur's toolset, without everyone recompiling the exe.

This is a design doc to steer how we build, so we don't paint ourselves into a
corner. Nothing here is implemented yet — it's the target.

## The hard constraint

`AGENTS.md` bar #1: **never break window management.** That single rule decides the
architecture. A mod author's bug must NOT be able to crash the hook thread, stall
the manager, or lose a window. So:

- **Code mods run OUT OF PROCESS** (separate exe talking to Astur over IPC). A
  crashing mod takes down the mod, never the WM.
- **In-process extension is data-only** (declarative config), or — much later and
  cautiously — a sandboxed embedded script with no access to the hot paths.

This also keeps the core tiny and the single-TU build fast (`CLAUDE.md`): mods live
beside the exe / in `~/.astur/mods/`, not in `main.rs`.

## Three tiers (ship in this order)

### Tier 1 — Declarative mods (data, no code) — closest to now

A mod is a folder under `~/.astur/mods/<name>/` (or a portable `./mods/<name>/`
next to the exe) with a `mod.toml` manifest + assets. Astur loads them at startup
and on hot-reload (the config watcher already exists). Declarative mods can provide:

- **Themes** — colours, bar palette, launcher palette, border colours, fonts.
- **Keybind sets** — remap/extend the hotkey table (already parsed in `config.rs`).
- **Window rules** — per-app float/ignore/workspace assignment (extends the existing
  `float_classes`/`ignore_classes` model).
- **Launcher entries** — extra fixed launcher items (custom commands, URLs, scripts
  to ShellExecute), and later launcher *source* toggles.
- **Bar layout** — which widgets, order, format strings (extends the bar config).

Manifest sketch:

```toml
[mod]
name = "nord-theme"
version = "1.0.0"
author = "..."
kind = "theme"          # theme | keybinds | rules | launcher | bar | bundle

[theme]
launcher_bg = "#2E3440"
launcher_sel = "#5E81AC"
border_focused = "#88C0D0"
# …maps onto existing Config fields
```

Implementation seam: `config.rs` stays the pure parser; add a `mods.rs` (also
Win32-free) that reads enabled mod manifests and **merges** them into `Config` (mod
overrides base, last-enabled wins, deterministic order). Keep it testable like
`config.rs`/`layout.rs`. No Win32 → no risk to the WM.

### Tier 2 — Out-of-process IPC mods (code, sandboxed) — the real extensibility

This is how third parties "extend the toolset" in any language. A mod is its own
process that connects to Astur over a **local named pipe** (`\\.\pipe\astur`) and
speaks a small line/JSON protocol:

- **Commands (mod → Astur):** essentially the existing `Cmd` enum exposed as a
  stable wire API — `switch_workspace`, `focus_dir`, `move_to_ws`, `retile`,
  `launch`, `close_focused`, `toggle_float`, query state (`list_windows`,
  `get_layout`), etc. Astur validates + pushes onto `CMDQ` (the manager already
  serialises everything safely).
- **Events (Astur → mod):** a subscribe stream — `window_added`, `window_focused`,
  `workspace_switched`, `monitors_changed`, `drag_ended`. Sourced from the existing
  win-event/manager points.

Why out-of-process: a mod can hang or crash with zero effect on the hook/manager
threads. It's language-agnostic (Python, JS, Rust, a shell script with a pipe
helper). This is the model komorebi uses (`komorebic` + a socket) and it's proven.

Examples this unlocks: a custom launcher/scratchpad, an auto-tiling-rules engine, a
workspace-naming HUD, presence integrations, a "focus mode" toggler.

Build seam: factor a thin `ipc.rs` worker thread (own the pipe, its own message
loop) that translates wire messages ↔ `Cmd`/events. The manager gains an event
broadcast hook (cheap: push to subscriber queues). Nothing on the input hot path.

### Tier 3 — Embedded scripting (maybe, later)

For in-process logic without spawning a process, embed a **pure-Rust sandboxed**
interpreter (e.g. Rhai) exposing the same command/event surface as Tier 2, run on a
dedicated worker (never the hooks/manager). Lower priority — Tier 2 covers most
needs more safely. Only pursue if users want lightweight logic mods without shipping
a binary. Hard rule stays: the script VM never touches the hook/manager threads
directly; it enqueues `Cmd`s like everyone else.

## Code-modularity work that enables all of this (do alongside features)

Keep the single `main.rs` TU (build speed), but carve clear seams:

1. **Launcher = providers.** Refactor the picker around a `ResultProvider` notion:
   `apps` (Phase 2), `files` (Phase 3), and future `calc` / `web` / mod-supplied
   providers. Each yields `(score, Result)` for a query; the launcher merges +
   ranks. This is the single most "extend the toolset" lever and Phase 3 should be
   built provider-shaped so mods can add sources later.
2. **Stable `Cmd` + a typed `Event`.** The `Cmd` enum is already the one funnel into
   the manager — treat it as the public API surface for Tier 2. Add a parallel
   `Event` enum + a broadcast list the manager notifies.
3. **`Config` merge layering.** Base config → enabled declarative mods → user
   overrides, computed in the Win32-free `config.rs`/`mods.rs` so it's testable.
4. **Asset loading** (icons/themes from a mod folder) reuses the launcher icon path.

## Non-goals / guardrails

- No in-process native DLL plugins (LoadLibrary of third-party code into the WM
  process) — one bad DLL crashes window management. Out-of-process only for code.
- Mods never get a handle to the hooks or `SetWindowPos`. They enqueue intents.
- Declarative mods can't express anything that bypasses the manager.

## Security model — "can people make malicious mods?" (discussed 2026-06-27)

Honest answer first: **any plugin system that can run code or commands cannot be made
100% malware-proof** — VS Code extensions, OBS plugins, browser extensions all have
this exact problem. You don't make it impossible; you make it **safe by default,
gated, and isolated**, and you provide a **trusted source** for the verified ones.

### Should the exe just run a `mods/` folder next to it?

- **Folder discovery: yes.** Check `./mods/` next to the exe (portable — drop exe +
  mods on a USB, matches Astur's ethos) AND `%USERPROFILE%\.astur\mods\` (installed).
- **"Run any mods in there": NO — not as code.** That is the trap. If "running a mod"
  means executing its code in Astur's process, one bad mod = arbitrary code in your
  WM, game over. So the default mod is **data, not code** (Tier 1): it can only *edit
  config through a validated schema* (themes, keybinds, rules, menu/launcher entries).
  Loading it never executes anything.

### The five guardrails (defense in depth)

1. **Data-only by default.** Tier-1 mods are declarative and schema-validated. Loading
   one can change colours/keybinds/rules/entries — nothing else. No code path.
2. **No execution at load — ever.** A mod can *register* a user-triggered action
   (a keybind or a menu/launcher entry that runs a command/URL), but Astur **never
   auto-runs** it. It fires only when the user explicitly picks/presses it — same trust
   level as the user making a shortcut themselves. A mod that's merely present can do
   nothing.
3. **Capability manifest + one-time consent.** Each mod's `mod.toml` declares what it
   touches: `theme` / `keybinds` / `rules` / `run-commands` / `ipc`. The cosmetic caps
   are silent. The moment a mod wants `run-commands` or `ipc`, Astur shows a one-time
   consent prompt ("This mod can run programs you trigger / talk to Astur. Enable?")
   and remembers the decision keyed by a **hash of the mod** — change the mod, re-ask.
   Browser/Android-style permissions.
4. **Out-of-process for real code (Tier 2).** Code mods are separate processes over the
   named pipe — a crash/hang takes down the mod, not the WM, and it runs with the
   user's normal OS rights (no extra grant). Later: launch them in a restricted token /
   AppContainer for actual sandboxing.
5. **Signing + a curated registry = the trust layer.** Official/“verified” mods we
   publish are **signed** with our key and come from a curated download site we review
   (like a store). Astur labels mods **Verified** (signed, from the registry) vs
   **Unverified** (everything else) and requires an explicit enable for unverified ones.
   This is where "make sure people can't ship malware" actually lives: not in code
   that's impossible to fully sandbox, but in *curation + signing + clear warnings*.

### What this means concretely

- Cosmetic mods (themes, layouts, bar): zero risk, no prompt.
- Mods that run commands/URLs you trigger: gated behind consent, never auto-run.
- Third-party code mods: out-of-process, unverified-by-default, explicit enable,
  eventually sandboxed.
- "Verified" mods: signed by us, reviewed, from our registry — the safe default catalog
  to publish and recommend.

We can't promise "no one can ever write a malicious mod." We CAN promise: a malicious
mod can't run just by existing, can't crash window management, can't act without the
user triggering it, and is clearly flagged Unverified unless it came through our signed
registry.

## Code edits we'll need (concrete)

### Tier 1 — declarative mods (build this first)

1. **New `src/mods.rs` (Win32-free, unit-testable like `config.rs`/`layout.rs`).**
   - Discover mod folders: `./mods/<name>/` (next to the exe, portable) + a
     `%USERPROFILE%\.astur\mods\<name>\` walk. Each has `mod.toml`.
   - Parse `mod.toml` → a `Mod { name, version, kind, enabled, capabilities, … typed
     override fields }`. Reuse the existing key/value parser style in `config.rs` (or
     add a tiny TOML reader — keep deps minimal; a hand parser is fine for flat keys).
   - `pub fn load_mods() -> Vec<Mod>` and `pub fn apply_mods(cfg: &mut Config, mods:
     &[Mod])` that layers overrides deterministically (base → each enabled mod in a
     stable order; last wins). No Win32 — testable.
2. **`src/config.rs`**: after building the base `Config`, call `mods::apply_mods`. Add
   new `Config` fields mods can populate:
   - `launcher_entries: Vec<LauncherEntry { name, exec, icon }>` — extra fixed picker
     items (mod-supplied apps/commands/URLs).
   - `sys_actions_extra: Vec<SysActionDef { label, exec, confirm }>` — extra power-menu
     rows.
   - mod keybinds: `mod_keys: Vec<(vk, shift, Command)>`.
   - (theme/border/bar colours + the launcher colours already belong in `Config` —
     see #4.)
3. **`src/main.rs` launcher**: in `launcher_enumerate`, after apps, append
   `cfg.launcher_entries` as `AppEntry`s (a mod entry whose `exec` is a command/URL →
   `ShellExecuteW`). The picker is already `Hit::App|File`; mod entries ride the App
   path. (Provider-ify later so file/web/mod sources are pluggable.)
4. **`src/main.rs` theme from config**: the launcher/sysmenu currently hardcode
   `LAUNCHER_*` colour consts. Move them into `Config` (read by the launcher/sysmenu
   threads via statics, like the bar already does with `apply_bar_statics`) so a theme
   mod can recolour them. New `apply_launcher_statics(cfg)`.
5. **`src/main.rs` sysmenu**: make `SYS_ACTIONS` a runtime `Vec` built from the
   built-in defaults + `cfg.sys_actions_extra`, not a `const`. A mod action’s `exec`
   runs via `ShellExecuteW` (still user-triggered).
6. **Mod keybinds**: add `Cmd::RunCommand(String)` and have `resolve_hotkey` consult a
   `MOD_KEYS` map (built from `cfg.mod_keys`) → push `Cmd::RunCommand`. The manager
   runs it via the existing `launch()` helper. (User-triggered only — never auto-run.)
7. **Hot-reload**: `config_watcher` already posts `WM_RELOAD`. Extend the manager’s
   reload to re-run `load_mods` + re-apply statics so theme/entry/keybind changes land
   live. Also watch the `mods/` dirs (mtime) the same way the conf files are watched.
8. **Consent + capability gate** (when a mod declares `run-commands`/`ipc`): a tiny
   owner-drawn consent popup (reuse the launcher/sysmenu scaffolding) on first load of
   such a mod; store approvals as `{mod_hash: caps}` in `~/.astur/mods/consent.json`;
   re-prompt when the hash changes. Cosmetic-only mods skip this.

### Tier 2 — out-of-process IPC mods (command transport shipped; events/providers later)

2026-07-16: opt-in local named pipe now accepts whitelisted commands for workspace,
layout, focus, launch, scratchpad, launcher/menu, reload, and status. Remote clients
are rejected. Event subscriptions, capability consent, provider streaming, and a
bundled client SDK remain later work.

- **New `src/ipc.rs`**: a worker thread owning a named pipe (`\\.\pipe\astur`).
  Reads line/JSON requests → validates → `push_cmd(Cmd)`. Exposes the existing `Cmd`
  surface as the wire API.
- **`Cmd`/manager**: add a parallel `Event` enum + an `EVENT_SUBSCRIBERS` broadcast
  list; the manager notifies on window add/focus/close, workspace switch, monitors
  changed (cheap pushes to subscriber queues). The IPC worker forwards events to its
  pipe clients.
- Nothing on the hook hot path; a mod hang/crash is its own process.
- Later: launch trusted/registered mods in a restricted token / AppContainer.

### Guardrail in code

Wherever a mod value can `exec` something, it goes through `ShellExecuteW`/`launch()`
on a user action only — there is no code path that runs a mod-supplied string at load,
on a timer, or off an event. Keep it that way.

## Status / next step

Notes only. First concrete step when we act on this: build Phase 3's file search as
a **provider** (item 1 above) so the launcher is already pluggable, then land Tier 1
(declarative theme/keybind/rule mods) since it reuses `config.rs`. Tier 2 (IPC) is a
separate, bounded piece after that.
