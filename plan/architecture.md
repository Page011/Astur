# Architecture

How Astur is put together. Pair with the function map in `src/main.rs`.

## One-process, many single-purpose threads

State is owned by exactly one thread and reached by the others over
`Mutex`/`Condvar` queues or atomics. There is no shared mutable window state
across threads except through these channels.

```
        OS input stream
              │
   ┌──────────┴───────────┐
   │  WH_MOUSE_LL hook     │  mouse_proc   ── fast, no work ──┐
   │  WH_KEYBOARD_LL hook  │  keyboard_proc ── fast, no work ─┤
   └───────────────────────┘                                 │
                                            push Cmd ─────────▼
                                         CMDQ (Mutex<VecDeque<Cmd>>)
                                                 │ CMDCV.notify
                                                 ▼
                                          manager_loop  (owns Manager:
                                          monitors → workspaces → windows)
                                          all SetWindowPos / tiling / switch
                                          │            │              │
                                                       │              └─ dispatch_slide
                                                       ▼                     ▼
                                                style/border/focus      transition_worker
                                                                        (overlay compositor)
```

Drag (Alt-move / Alt-resize) never touches the real window from the hook. The
preview is a live **DWM thumbnail** overlay (`THUMB_HWND`, `DwmRegisterThumbnail`;
falls back to the outline frame `OUTLINE_HWND` if registration fails) that follows
the cursor. On the thumbnail path the hook pushes `Cmd::DragPark` and the MANAGER
parks the real window off-screen (only the mirror is visible); on button-up the
hook pushes `Cmd::DragMoved(h, x, y, rect)` / `DragResized(h, Some(rect))` and the
manager `commit_rect`s the previewed rect (one cross-process `SetWindowPos`), then
re-tiles. All cross-process window placement lives on the manager thread. There is
no `position_worker` thread any more (removed 2026-07-07; park/commit moved fully
off the hook 2026-07-10).

The popups take mouse input: the hook routes wheel + click-outside (rect atomics
published by `launcher_place`/`sysmenu_layout`); hover-select and click-activate
are handled in `launcher_wndproc`/`sysmenu_wndproc` directly (the windows are
NOACTIVATE but still receive mouse messages).

Side threads, independent: `stats_worker` (CPU/RAM/battery ~2s), `config_watcher`
(file mtime → `WM_RELOAD`), `focus_follow_worker`, and one message-pump window per
monitor for the status bar (`bar_wndproc`).

`focus_follow_worker` polls the cursor every 16ms (~1 frame) for a snappy hover,
but only runs the expensive `WindowFromPoint` + `MANAGED` lock when the cursor
actually moved since the last tick (cached `last_pt`). A still cursor costs one
`GetCursorPos` and bails — so the fast poll doesn't burn idle CPU. Drives focus
via a `Cmd::FocusMouse` push, never a direct `SetWindowPos` off this thread.

Because the poll is fast, it would otherwise fight keyboard focus changes: right
after a workspace switch the cursor may sit over a window elsewhere, and the
worker would yank focus back. Guard: the manager calls `bump_follow_settle()` on
every programmatic focus (`switch_monitor_workspace`, `FocusDir`, `FocusGeo`),
which sets `FOLLOW_SETTLE_MS` to `now_ms() + 200`. The worker skips (and re-syncs
`last_pt`) while inside that window, so the programmatic focus wins and only a
real cursor move after it expires re-engages follow.

## Why the hooks do nothing

`mouse_proc` is on the hottest path in the OS — every mouse move system-wide
passes through it. Rules:

- Early-out on `ANY_DRAG`/`ALT_DOWN` atomics **before** taking any lock, so the
  no-drag case is a couple of atomic loads.
- Never do a *cross-process* `SetWindowPos` on the hook — not even once per drag.
  The preview overlays are our own windows (cheap); the real window's park and
  final placement go through `Cmd::DragPark`/`DragMoved`/`DragResized` and run on
  the manager thread (`commit_rect`). The old per-frame `position_worker` +
  `set_target` path was removed — resizing a foreign window live stalls on its own
  repaint (see `known-issues.md` 2026-07-07).
- `keyboard_proc` swallows Left Alt entirely (`SUPPRESS`/`FAKE_ALT` dance keeps
  Alt+Tab working with a synthetic Alt).

If you add anything to a hook proc, measure it. A 1 ms stall here is felt as
system-wide input lag.

## Manager state

```
Manager
├── monitors: Vec<Monitor>
│   └── Monitor { hmon, work_area, active: usize, workspaces: Vec<Workspace> }
│       └── Workspace { windows: Vec<isize>, floating: Vec<isize>, focused, splits }
├── tiling: bool
├── primary, focused_mon: usize
└── cfg: Config
```

- Windows are stored as raw `isize` HWNDs; `hwnd_from()` rebuilds the typed handle.
- `MANAGED` / `INDEX` statics mirror membership for the win-event/enum paths.
- Workspaces are never cleared on switch — only `ShowWindow(SW_HIDE/SW_SHOWNA)`.
- Layout geometry is pure (`layout.rs`): `dwindle_layout` and `master_stack` take
  a work area + count and return rects. No Win32 in there — easy to test/reason.

## Tile placement is INSTANT (important)

`animate_to` is a historical name — it places instantly (`set_pos_raw` = one
`SetWindowPos` with `SWP_NOACTIVATE|NOZORDER|NOSENDCHANGING`). Per-window
interpolated glide was tried and **removed**: see `animations.md`. Do not
reintroduce naive per-frame `SetWindowPos` interpolation.

## The workspace-switch overlay (the one real animation)

`switch_monitor_workspace` does the real switch instantly and correctly, then
hands a *cosmetic* snapshot to `transition_worker`:

1. Freeze outgoing workspace to an HBITMAP (`capture_monitor`) + its window rects.
2. Pull the incoming workspace's last snapshot (`snap_get`) if we have one.
3. `dispatch_slide(SlideReq{..})`; worker raises a topmost `WS_POPUP` overlay
   showing frame 0 (== current screen, no visible change) and signals back.
4. Manager does the real (now hidden) switch underneath the overlay.
5. Worker composites frames (GDI BitBlt of out/in over a captured wallpaper) and
   tears the overlay down at the end, revealing the already-correct real windows.

Snapshots are GPU-backed DDBs (`dup_ddb`), owned by whoever the comment says —
the worker always gets private copies. First visit to a workspace has no snapshot,
so its first entry is an instant (no-overlay) switch.

This overlay is the extension point for new transition modes — see
`animations.md`.

## Config flow

`config.rs` parses both files to a `Config`. `apply_bar_statics` pushes bar-related
values into atomics the bar thread reads. Hot-reload: watcher posts `WM_RELOAD`;
manager rebuilds `Config`, re-applies statics, re-tiles. Keep `config.rs` Win32-free
so it stays trivially correct.
