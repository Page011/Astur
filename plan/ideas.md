# Ideas / backlog

Loose. Promote to a real plan doc or a commit when picked up.

## Animation
- [ ] Workspace modes: `off | slide | spring | fade` (in progress — `animations.md`).
- [x] Per-window snapshot-glide (Option A) for open/close/move/resize — SHIPPED
      (`window_anim = glide`, `run_window_glide`). See `animations.md`.
- [ ] `spring` option for window glide (`ease_out_back`) — foundation now exists.
- [ ] Multi-monitor simultaneous glide (one overlay slot today → only one monitor
      glides on a multi-mon retile).
- [ ] Per-event curve config like Hyprland (style name + duration + bezier).
- [ ] Fade variant that also drifts a few px (directional fade).

## Perf
- [ ] Profile `mouse_proc` no-drag path — confirm it's atomics-only, no surprise lock.
- [x] ~~Batch retile via `DeferWindowPos`~~ — REJECTED (see `known-issues.md`):
      a defer batch fails wholesale if one window misbehaves. Robustness wins.
- [ ] Bar repaint: ensure double-buffered, only invalidate changed regions.
- [ ] Stats worker poll interval / cost — confirm ~2s and cheap.

## Features
- [ ] Optional socket/CLI control (komorebi-style) for scripting.
- [ ] More layouts (columns, grid) — keep `layout.rs` pure.
- [ ] Named/persistent workspaces.
- [ ] Per-monitor independent animation settings.

## Docs / honesty
- [ ] Reconcile README animation claims with reality (see `known-issues.md`).
- [ ] Short CONTRIBUTING pointing at AGENTS.md + plan/.

## Open questions
- Should window-glide be on by default once built, or opt-in given the capture
  cost on open? Lean opt-in until proven cheap on low-end GPUs.
- DeferWindowPos vs per-window: does batching actually reduce visible reflow, or
  just call count? Needs a measurement.
