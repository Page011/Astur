# Launcher — app picker + (planned) file search

Omarchy/rofi-style centered picker. Type to fuzzy-filter, arrows to select,
Enter to launch, Esc to dismiss. Trigger: **Alt+Space** (Left Alt is already
Astur's reserved modifier, so no system-shortcut conflict — deliberately NOT
Win+Space, which is the Windows layout toggle).

## Scope

| Phase | Source | Status |
|---|---|---|
| v1 | Start Menu `.lnk`/`.url` shortcuts (installed apps) | **SHIPPED 2026-06-26** |
| v2 | + shell `AppsFolder` (UWP/system apps: Notepad, Calculator, …), per-row icons, click-outside-to-dismiss | **SHIPPED 2026-06-26** |
| v4 | Tab **wide column view** (Modified/Size/Path), full **mouse support** (hover/click/wheel), icon pipeline v3 | **SHIPPED 2026-07-10** |
| v5 | **inline calculator** + **web-search fallback** rows; double-buffered paint (no icon flash on scroll); theme-aware palette | **SHIPPED 2026-07-13** |
| later | Windows Search index (files, Everything-style) | planned (below) |
| next | clipboard-history prefix, emoji-picker prefix (queued — `roadmap-v2.md`) | queued |
| maybe | open-window switcher (focus a managed window) | backlog |

### v5 as built (2026-07-13)

- **Inline calculator**: a maths-looking query (`5*7+2`, `(1+2)^3`, calc chars only,
  ≥1 digit + ≥1 operator — so app names never trigger it) pins `Hit::Calc` as the
  first row showing `= result   (Enter copies)`; Enter copies via
  `clipboard_set_text` (CF_UNICODETEXT; the copy happens BEFORE `launcher_close`
  so the popup still owns the clipboard call). Recursive-descent `calc_eval`
  (+ - * / % ^ parens unary-minus, div/mod-by-zero and trailing-garbage → None).
- **Web-search fallback**: a non-empty query with NO app/file/calc matches shows a
  single `Hit::Web` row (`Search the web for "…"`); Enter opens the default
  browser via a percent-encoded Google search URL. Note: it can appear for
  ~100-300ms while the async file search is still in flight — accepted (apps match
  instantly in the common case).
- **No more scroll flash**: paint goes through `backbuf_begin`/`backbuf_end`
  (memory DC, one BitBlt) and `LA_SCROLL` skips repaints that change nothing.
- **Theme**: the palette is `pal()` (dark/light/auto from `theme=` in astur.conf),
  read at paint time so a hot-reload retints the popup live. Acrylic (opt-in) is
  applied on every `launcher_show`.

### v4 as built (2026-07-10)

- **Tab = wide column view** (replaces the old detail footer): the picker resizes
  660→1060px (clamped to the work area, recentered via `launcher_place`, bounds
  republished) and file rows gain Modified / Size (right-aligned) / Path columns
  with a dim header row; app rows show their launch path. `fmt_oadate`/`fmt_size`
  formatters reused.
- **Mouse**: hover moves the selection (screen-space move guard so a popup opening
  under a still cursor can't steal it — `LAUNCHER_LAST_MX/MY`), click activates the
  row (same path as Enter), wheel scrolls the viewport (`LA_SCROLL` posted from the
  LL mouse hook, which owns wheel routing; selection is clamped into view so Enter
  always acts on something visible). Scrolling is now explicit `st.scroll` state —
  it no longer derives from `sel`, which would have made hover jump the list.
- **Icon pipeline v3** (`load_icon`): exact-size `IShellItemImageFactory::GetImage`
  → HICON (HQ shell scaling, UWP-capable) → fallback `SHIL_LARGE` native 32px →
  cached generic .exe icon (rows never blank). Drawn 1:1 with `DrawIconEx`. See
  `known-issues.md` 2026-07-10 for the jumbo regression this replaced.

### v2 as built (2026-06-26)

Closes the "Notepad isn't there" gap and adds icons + mouse-dismiss.

- **App coverage**: `launcher_enumerate` now also walks the shell `AppsFolder`
  (`shell:AppsFolder`, the "All apps" list) via `IShellItem`/`IEnumShellItems`
  (`enumerate_appsfolder`). That pulls in UWP + system apps with no Start Menu
  `.lnk` — Notepad, Calculator, Settings, Store apps. `.lnk` entries are inserted
  first and win the name dedup (their launch is rock-solid); AppsFolder fills the
  rest. Launch token per entry: the `.lnk`/`.url` path, OR `shell:AppsFolder\<id>`
  (the item's `SIGDN_PARENTRELATIVEPARSING`, normally an AUMID), OR — when that id
  is a real exe path — the path itself. All three launch through the existing
  `ShellExecuteW(open, …)`. Verified on a Win11 box: 174 apps, Notepad +
  Calculator present, Notepad id = `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`.
- **Icons** (omarchy-style): per-row 24px app icon. Resolved lazily off the UI
  thread by `icon_worker` (`SHCreateItemFromParsingName` → `IShellItemImageFactory
  ::GetImage`, a 32bpp ARGB HBITMAP), cached on the `AppEntry`, drawn with
  `AlphaBlend` (per-pixel alpha). `launcher_paint` enqueues only the visible rows
  that lack an icon (`ICON_QUEUE`, deduped); failures cache as -1 (no retry).
- **Click-outside-to-dismiss**: the picker is `NOACTIVATE` (never focused), so the
  global `mouse_proc` is the only place that sees the click. It publishes the
  picker bounds (`LAUNCHER_RECT_*` atomics) on show; a button-down outside them
  posts `LA_CLOSE` and is swallowed. Hook stays light: one atomic load when the
  launcher is closed (the common case).
- COM (`CoInitializeEx`, STA) is set up once on both the launcher thread (enum) and
  the icon worker. The launcher thread enumerates at startup (idle before first
  open) so Alt+Space is instant.

### v1 as built (2026-06-26)

Matches the design below. Lives in `src/main.rs` (launcher section before
`main`). Trigger `Alt+Space`; driven entirely via the LL keyboard hook posting
`WM_LAUNCHER` to the picker window (no foreground focus needed). State in
`LAUNCHER_STATE`, mutated only on the launcher thread's wndproc. Enumerate =
`launcher_enumerate` (both Start Menu roots, dedup per-user over all-users);
match = `fuzzy_score` (subsequence + boundary/run bonuses); launch =
`ShellExecuteW` on the shortcut. GDI-rendered, Forte-blue selection.

Known v1 gaps / backlog (none block use):
- No click-to-select / click-outside-to-close (window never takes focus; Esc
  closes). Mouse support is a follow-up.
- No icons (text only).
- Raw VK→char map (`MapVirtualKeyW`), so shifted symbols / IME aren't captured —
  fine for filtering app names.
- App list cached on first open for the process lifetime; no manual refresh key
  yet (restart picks up newly installed apps).
- `config.rs` keys (enable/colours/size/trigger) not wired yet — hardcoded v1.
  Constants live at the top of the launcher section.

### v2.1 polish (2026-06-27)

- **Icons load fast + fully.** Was: one icon worker, only visible rows enqueued —
  slow, and rows you didn't scroll to never got an icon. Now: **3 parallel** COM
  icon workers, and the whole app list is **preloaded** at startup (enqueued after
  enumeration) so it's fully iconned before first open. On-paint enqueue stays as a
  fallback (deduped). Worker count is a speed/RAM trade (see `optimization.md`).
- **Professional look**: DWM **rounded window corners**, a **rounded accent
  selection pill** (inset, `RoundRect`) instead of an edge-to-edge bar, a thin 1px
  frame (was a heavy 2px blue border), muted divider, larger icons (28px) + rows
  (40px) + padding, and a caret in the query row. Colours unchanged in spirit
  (Forte blue accent on dark).

## Phase 3 — file search, detail view, open-folder — SHIPPED 2026-06-27

Built as designed. As-built notes:

- **Backend**: OLE DB `Search.CollatorDSO` (Windows Search index). `FileSearch` opens
  one connection on a dedicated `filesearch_worker` (own COM STA) and reuses the
  session; each query is a fresh command. Bindings: path `WSTR|BYREF` (provider ptr),
  size `I8`, date automation `DATE` — read in one rowset pass (`IRowset`/`IAccessor`/
  `DBBINDING`, ported from a verified scratchpad probe). Non-file rows (Outlook items)
  dropped via `is_fs_path`. Query sanitised for `LIKE` (`sanitize_like`, `ESCAPE '\'`).
- **Debounce + cancel**: typing posts `(gen, query)` to `SEARCH_REQ`; the worker waits
  120ms, bails if `SEARCH_GEN` moved on, runs the query, and bails again if superseded
  before applying — so a fast typist never backs up the index. `st.files` cleared on
  each keystroke so stale results never linger.
- **Results**: merged into the picker as `Hit::App | Hit::File` — apps (instant,
  fuzzy) first, then up to 40 file hits. File rows show the filename (+ a small marker;
  per-extension shell icons are a backlog item).
- **Tab** toggles a detail footer for the selected file: full path, `Modified`
  (formatted from the automation date), `Size` (human-readable), and the key hints.
- **Enter** opens the file; **Shift+Enter** opens its containing folder
  (`explorer /select,"path"`). Both via the LL keyboard hook posting `LA_TAB` /
  `LA_ACTIVATE` / `LA_ACTIVATE_ALT`.
- Degrades safely: if the index can't be opened, `FileSearch::new` returns `None` and
  file search is silently disabled (apps still work).

Backlog: per-extension file icons (reuse the shell-icon path), AQS operators (`kind:`,
`ext:`), scope/ranking tuning, provider-ify so mods can add result sources.

### Original Phase 3 plan (for reference)

User asks still open after v2:
1. **File search** in the picker ("like Everything"), to also replace Win+search
   for files/folders.
2. **Tab to expand** a result to show file path, modified date, size (omarchy-style
   "press Tab twice" = open the detail view, Tab again to collapse).
3. **Shift+Enter** opens a file result's containing folder instead of the file.

### Decision (from user, 2026-06-26)

Backend = **Windows Search index** (the `SystemIndex` catalog, Option A below).
Rationale: user constraint was "minimal RAM and FAST" — the OS already maintains
this index, so Astur holds ~no file table of its own (vs the MFT/USN approach,
which is tens of MB resident and needs admin). It's the same source Start/Explorer
search use, so it matches "replace Win+search." Covers indexed locations only
(acceptable; user can widen via Indexing Options). No admin.

### UI / interaction

- Default list = app matches (instant, in-memory) as today. When the query is
  non-empty, **also** run a file-index query and append file results below the
  apps (or interleave by score). Apps stay first so the common case never waits.
- **Debounce + cancel**: file query fires ~120ms after the last keystroke on a
  dedicated worker; a newer keystroke supersedes an in-flight query (cancellation
  token / generation counter) so a fast typist never backs up the index.
- **Tab** toggles an expanded detail view for the selected (file) result: a taller
  row / side panel showing full path, `System.DateModified`, `System.Size`. Tab
  again collapses ("twice" = open then close). Apps ignore Tab (no extra detail).
- **Enter** launches (app: ShellExecute; file: open the file). **Shift+Enter** on a
  file opens its containing folder — `ShellExecuteW("open", parent_dir)` or
  `explorer /select,<path>` to highlight it.

### Result model + RAM

- Generalise `AppEntry`/`filtered` to a result enum: `App { … }` |
  `File { name, path, modified, size }`. Only the **top N** (≈50) file rows per
  query are held; discarded on the next query. No persistent file table → the
  "minimal RAM" bar holds.
- Keep app icons; file rows get a generic file/folder icon (or per-extension via
  the same `IShellItemImageFactory` path, lazily — reuse `icon_worker`).

### Backend wiring — DE-RISKED 2026-06-27 (verified on this machine)

Backend CONFIRMED = **OLE DB `Search.CollatorDSO`** (the Windows Search index).
WSearch is running; the index is rich + fast. Verified working query (PowerShell ADO
against the live index returned real paths/sizes/dates):

```sql
SELECT TOP 50 System.ItemPathDisplay, System.ItemNameDisplay,
       System.Size, System.DateModified
FROM SYSTEMINDEX
WHERE System.FileName LIKE '%<q>%'
ORDER BY System.DateModified DESC
```

- Connection string: `Provider=Search.CollatorDSO;Extended Properties='Application=Windows'`.
- `System.FileName LIKE '%q%'` matches well and returns path/size/modified as columns
  in ONE query — **no per-item `IShellItem2` needed** for the Tab detail view. Simpler
  than the shell route.
- **REJECTED dead end:** the `search-ms:` shell protocol +
  `IShellItem.BindToHandler(BHID_EnumItems)` returns **0 items** here (tried
  `ext:.lnk`, `*.txt`, name terms, with an async-populate retry). Do not use it; use
  OLE DB. (Logged in `known-issues.md`.)
- **Caveat — filter non-file rows:** the index also returns Outlook/mail items whose
  path looks like `/account@dom/Folder/Subject.pdf` (no drive). For a file launcher,
  keep only rows whose path is a real filesystem path (`^[A-Za-z]:\` or `^\\`), or
  scope by `System.ItemType`. Cheapest: post-filter on path shape.
- **Escaping:** sanitise the query before interpolating into `LIKE` — escape `'`
  (→ `''`) and the wildcards `%`/`_` (with an `ESCAPE` clause) to avoid garbage matches.

### Rust binding — prototype in the probe FIRST, then port

OLE DB lives in the `windows` crate under **`Win32::System::Search`** (feature
`Win32_System_Search`): `IDataInitialize` (CLSID `MSDAINITIALIZE`) → `GetDataSource`
→ `IDBInitialize::Initialize` → `IDBCreateSession::CreateSession` →
`IDBCreateCommand::CreateCommand` → `ICommandText::SetCommandText` → `ICommand::Execute`
→ `IRowset`, then `IAccessor::CreateAccessor` + `DBBINDING[]` + `GetNextRows`/`GetData`
(~150–200 lines of dense unsafe). Alternative: late-bound `IDispatch` on
`ADODB.Connection` (what the PowerShell test used; VARIANT-heavy, ~100 lines).
**Build whichever in the scratchpad probe and match the PowerShell results before
transplanting into `main.rs`** — the WM must never get an untested COM binding
(correctness-first). Then run it on a dedicated `filesearch_worker` (own STA),
debounced ~120ms with a generation/cancel guard, posting top-N results back +
`InvalidateRect`, exactly like `icon_worker`. Build the launcher result list as a
provider (apps | files) so it's mod-pluggable later (see `plan/mods.md`).

### Threading

New `filesearch_worker` (CoInitialize STA, own request slot + generation counter).
Launcher posts the current query on change (debounced); worker runs the index
query, writes results into `LAUNCHER_STATE` under a generation guard (drop stale),
`InvalidateRect`. Keyboard hook gains: Tab → `LA_TAB` (toggle detail), Shift+Enter
→ `LA_ACTIVATE_ALT` (open folder). Hook stays light (push intents only).

## v1 — app launcher

### Trigger (keyboard hook)

`keyboard_proc` already owns Left Alt (it's suppressed from apps). Add: when
`LAUNCHER_OPEN` is false and we see **Left Alt + Space** (VK_SPACE while
`ALT_DOWN`), push `Cmd::OpenLauncher` and suppress the key. Hook stays light —
no work, just a flag check + push (hooks are sacred, see `known-issues.md`).

When `LAUNCHER_OPEN` is true the hook switches to **capture mode**: it routes
typed keys to the launcher (query buffer) and swallows them from the system, so
normal Astur hotkeys and apps don't see them. Keys handled:

- printable (A–Z, 0–9, space, punctuation) → append to query
- Backspace → pop query
- Esc → close (push `Cmd::CloseLauncher`)
- Enter → launch selection (push `Cmd::LauncherActivate`)
- Up/Down → move selection (push `Cmd::LauncherMove(±1)`)

Capture mode must be careful: it runs in the LL hook, so it can only set atomics
/ push commands. The query buffer + selection live behind a `Mutex` owned by the
launcher thread; the hook pushes intent, the launcher thread mutates + repaints.
(Alternative considered: a hidden edit control to get real WM_CHAR text. Rejected
for v1 — a raw VK→char map avoids hosting a child control and keeps the window a
single owner-drawn surface. IME/dead-keys are a known v1 gap; revisit if needed.)

### Window + render

One topmost `WS_POPUP` layered window, centered on the focused monitor's work
area, ~640×420. Owns its own message pump + condvar idle, same pattern as the
slide compositor and bar threads. Created lazily on first open, then hidden /
reshown (cheaper than recreate).

GDI render (reuse the bar's font + double-buffer pattern):

- background: filled rounded rect, Forte-dark theme; 1px border (config colour).
- query row at top: the typed text + a caret.
- list below: one row per match, selected row highlighted. Show app display
  name; optionally the source path dimmed on the right.
- clamp to N visible rows, scroll the window around the selection.

Repaint triggers: open, query change, selection move. Debounce not needed at
human typing speed.

### App enumeration

Walk both Start Menu roots recursively for `*.lnk` (and `*.url`):

- all-users: `%ProgramData%\Microsoft\Windows\Start Menu\Programs`
- per-user: `%APPDATA%\Microsoft\Windows\Start Menu\Programs`

For each: display name = file stem (e.g. `Firefox.lnk` → "Firefox"). Dedup by
display name (per-user shadows all-users). Cache the list on first open; refresh
on a manual key (e.g. Ctrl+R) or when the launcher reopens after >N seconds.
Enumeration is pure FS walk — no COM needed.

**Launch**: `ShellExecuteW(verb="open", file=<lnk path>)`. Shelling the `.lnk`
directly resolves the target, working dir, args, and icon semantics for free —
no `IShellLink`/`IPersistFile` resolution required for v1. (Icon rendering, which
*would* need the shell APIs, is a backlog nicety — v1 is text-only.)

### Fuzzy match

Simple, fast, good enough:

1. case-insensitive subsequence test (all query chars appear in order).
2. score: prefer contiguous runs, start-of-word boundaries, earlier match, and
   shorter target. Stable-sort matches by score desc.

Pure function over `(query, candidate) -> Option<score>`, lives in a testable
spot. Keep it in `config.rs`/`layout.rs`-style Win32-free territory if practical
(it's pure string work) so it's unit-testable.

### Commands / threading

- `Cmd::OpenLauncher` → manager tells the launcher thread to show (centered on
  `focused_mon`), sets `LAUNCHER_OPEN=true`.
- `Cmd::CloseLauncher` → hide, clear query, `LAUNCHER_OPEN=false`.
- `Cmd::LauncherMove(i32)` / `Cmd::LauncherActivate` → launcher thread updates
  selection / launches + closes.
- Query edits: hook pushes char/backspace intents; launcher thread owns the
  buffer + recomputes matches + repaints.

Never block the manager on the launcher; never `ShellExecute` from a hook.
Activate launches on the launcher thread (or a throwaway) so a slow shell open
can't stall input.

### Config keys (astur.conf)

- `launcher = on|off` (default on)
- `launcher_key` (default `alt+space`) — parsed by `config.rs`
- `launcher_width`, `launcher_height`, `launcher_rows`
- colours: `launcher_bg`, `launcher_fg`, `launcher_sel`, `launcher_border`
  (default to the bar/border palette)

### v1 non-goals (backlog)

- icons (needs shell icon extraction)
- IME / dead-key input
- file search (separate phase, below)
- running-window switcher mode
- usage-frequency ranking / MRU

## Reference — file search backend options

DECIDED (2026-06-26): **Option A — Windows Search index** (see "Phase 3 plan"
above for the concrete build). The option analysis is kept below for context.

Windows ships a content index (**Windows Search**, the `SystemIndex` catalog) —
the same index File Explorer search and the Start menu use. It's queryable, so we
don't have to build our own like Everything does (Everything parses the NTFS
**USN/MFT** directly; that's the other option — faster + no index dependency, but
much more code and needs admin to read the volume).

### Option A — Windows Search via OLE DB (recommended)

Query the `Search.CollatorDSO` provider with SQL:

```sql
SELECT TOP 50 System.ItemPathDisplay, System.ItemNameDisplay
FROM SYSTEMINDEX
WHERE System.ItemNameDisplay LIKE '%query%'
ORDER BY System.DateModified DESC
```

- Connect: `provider=Search.CollatorDSO;Extended Properties='Application=Windows'`.
- From Rust: OLE DB is COM. Either use the `windows` crate's OLE DB bindings or
  shell out to a tiny helper. Simpler bridge: `System.Data.OleDb` is .NET — avoid
  pulling a runtime. Native path = `IDBInitialize`/`ICommandText` COM dance, or
  ADSI-style `IDispatch` on `ADODB.Connection` (heavier). Pick the lightest that
  works; prototype outside the hot path.
- Pros: reuses the OS index (instant, already maintained, respects indexed
  locations). Cons: only covers **indexed** folders (user can add locations via
  Indexing Options); COM/OLE DB boilerplate.

### Option B — USN journal / MFT scan (Everything's approach)

Read `\\.\C:` MFT + subscribe to the USN change journal to keep a live filename
index in memory. Pros: every file, not just indexed; very fast substring search.
Cons: needs admin (volume read handle), per-volume, significant code, must persist
+ replay the journal. Overkill for v1 of file search.

### Recommendation

Phase the file search as **Option A** behind the same launcher UI: a mode toggle
(e.g. prefix `>` or a hotkey) flips the source from apps to files, runs the
SYSTEMINDEX query on the launcher thread (debounced ~120ms after a keystroke,
since it hits the index), and renders results in the same list. Launch =
`ShellExecuteW` on the path. Keep app mode (instant, in-memory) as the default so
the common case never waits on the index. Revisit Option B only if users demand
non-indexed coverage.

### Open questions (file search)

- Lightest native OLE DB path from Rust without a .NET dependency? (prototype.)
- Debounce + cancellation: a fast typist outruns the index; cancel in-flight
  queries (new keystroke supersedes).
- Respect the user's Indexing Options scope; surface "folder not indexed" hint.
