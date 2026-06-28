# Competitors / prior art

Windows tiling WMs. What they do, what we learn/borrow, where the docs are.

## komorebi — https://github.com/LGUG2Z/komorebi

- Rust. Tiling window manager; pairs with a separate hotkey daemon (`whkd`) and
  optional status bars. BSP/columns/rows/stacks layouts, named workspaces, rules.
- Config: **required** (YAML/JSON-ish + CLI to drive a running instance via a
  socket). Powerful, scriptable, but setup-heavy.
- Docs: https://lgug2z.github.io/komorebi/
- **No animations.** Placement is instant. This is our motion-polish opening.
- Borrow: rules system depth, multi-monitor workspace model, socket/CLI control
  idea (Astur has none — possible future for scripting).

## GlazeWM — https://github.com/glzr-io/glazewm

- C++ (older) / now TypeScript+Rust (glzr-io ecosystem). i3-inspired. Tiling tree,
  workspaces, keybind-driven, has its own bar (Zebar).
- Config: **required** YAML. Installer-based.
- Docs / site: https://github.com/glzr-io/glazewm , https://glazewm.com
- **No real window animations.** i3 feel, instant tiling.
- Borrow: i3-style keybind grammar, container/tree mental model for advanced
  layouts (Astur is flatter — dwindle/master only).

## Seelen UI — https://github.com/eythaann/Seelen-UI

- Rust + Tauri (web UI). The user wrote "seleenUI" — correct name is **Seelen UI**.
- Much broader scope: tiling WM **plus** a full desktop environment shell (app
  launcher, themed toolbar/dock, widgets, wallpaper manager). Heavy, feature-rich,
  GUI-configured.
- Docs / site: https://seelen.io , releases on the GitHub repo.
- Has some UI animation (it's a web stack) but it's a different product class —
  a DE, not a tiny portable exe.
- Contrast: Astur is the *opposite* bet — one small native exe, zero setup, no web
  runtime. Don't chase Seelen's surface area; chase its polish at a fraction of the
  weight.

## PowerToys FancyZones — part of Microsoft PowerToys

- C#. Zone templates you snap windows into; not a true dynamic tiler (no auto BSP,
  no workspaces of its own — uses Windows virtual desktops).
- Docs: https://learn.microsoft.com/windows/powertoys/fancyzones
- Borrow: nothing structurally; it's the "zones, not tiling" baseline in our
  comparison table.

## Reference (not Windows) — Hyprland

- The motion benchmark the user keeps citing. Wayland compositor (it renders its
  own pixels, so it can animate freely — we cannot, see `animations.md`).
- Animation model worth copying *conceptually*: per-event bezier curves, named
  styles (slide/fade/popin), spring/overshoot on window open and workspace change.
- Docs: https://wiki.hyprland.org/Configuring/Animations/
- Lesson: expose animation **style names** + a duration/curve, like Hyprland's
  config — that's the UX we're matching even though our rendering path differs.

## Positioning (keep honest in README)

Astur trades configurability and breadth for **zero-setup + smallest-thing-that-
works + best motion**. The comparison table in `README.md` must stay accurate:
right now it claims "Silky smooth animations: Yes" — true only for the workspace
slide. Window-level glide is not shipped (see `known-issues.md`).
