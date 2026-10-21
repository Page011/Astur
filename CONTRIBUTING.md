# Contributing to Astur

Thanks for your interest in making Astur better!

Astur is a free, open-source tiling window manager for Windows, built in Rust. **Contributions are very welcome**, especially fixes and improvements for the [open issues](https://github.com/Page011/Astur/issues).

There is more on the to-do list than the maintainer can tackle alone, so community help makes a real difference.

You do not need to be an expert in Rust or Win32 to contribute. Bug fixes, tests, documentation, investigation, and small improvements are all useful.

## Find something to work on

1. Browse the [open issues](https://github.com/Page011/Astur/issues?q=is%3Aissue%20is%3Aopen).
2. Pick something that interests you and check the discussion for existing work.
3. Leave a short comment if you plan to tackle it, so others know you're looking into it.
4. **For a clearly defined existing issue, feel free to start coding without waiting for permission.** Open a draft pull request if you want to show progress or get early feedback.

For larger features, major design changes, or changes that alter existing behaviour, please open an issue first to discuss the approach. This helps avoid putting work into something that may not fit Astur's direction.

If you're not ready to write code, reproducing a bug, narrowing down its cause, testing a proposed fix, or improving instructions in an issue is still a valuable contribution.

## Get set up

You'll need:

- **Windows 10 or 11** for running and testing the window manager.
- [Rust stable](https://rustup.rs/) and Cargo.
- A working Windows C++ linker/build environment, such as Visual Studio Build Tools with C++ support, for the recommended MSVC toolchain.
- Git.

Fork the repository on GitHub, then clone your fork and create a branch:

```bash
git clone https://github.com/YOUR_USERNAME/Astur.git
cd Astur
git switch -c fix/short-description
```

Build the project from the repository root:

```bash
cargo build
```

To build the release binaries:

```bash
cargo build --release --workspace
```

The main workspace contains:

| Path | Purpose |
| --- | --- |
| `crates/astur` | Window manager, Windows hooks, layouts, animations, bar, and launcher |
| `crates/astur-config` | Shared configuration parsing and defaults |
| `crates/astur-settings` | Settings GUI |

Read [README.md](README.md) for features and usage, and [AGENTS.md](AGENTS.md) and [CLAUDE.md](CLAUDE.md) for codebase conventions and architectural guidance.

**Branch note:** `main` is the full Astur edition. Astur Lite is maintained separately on the [`lite` branch](https://github.com/Page011/Astur/tree/lite). Unless your change specifically targets Lite, base your pull request on `main`.

## Make your changes

- **Keep pull requests focused.** One bug fix or feature per PR is much easier to review than a broad rewrite.
- Follow the surrounding Rust style; use `cargo fmt` rather than formatting everything by hand.
- Add or update tests where practical, particularly for layout calculations and configuration parsing.
- Update the README and relevant `plan/` documentation when behaviour, configuration, architecture, or supported functionality changes.
- Don't add dependencies or restructure large parts of the codebase without a clear reason.
- Explain any important Win32 safety assumptions near the code that relies on them.

Astur's first priority is **reliable window management**. In particular:

- Keep global keyboard and mouse hooks fast. Do not add blocking work, unnecessary allocations, or expensive locking to input-hook paths.
- Actual window moves, tiling, and workspace changes must remain correct even if an animation or other visual effect fails.
- Preserve correct behaviour across multiple monitors, DPI scales, and unusual application windows where relevant.
- Ensure normal exit and recovery do not leave windows hidden or stranded off-screen.
- High quality window management above anything else. Users can use their own launcher, status bar, or shortcuts. Our first priority is keeping window management perfect.

## AI additions

Using AI or LLM's to write/refactor/edit your code is welcome. As long as you test it thoroughly and welcome pushback if there is further issues.

These are project-specific constraints, not barriers to helping. If you're unsure how to approach a fix, explain your idea in the issue or open a draft PR.

## Check your work

Before submitting, run the same checks as [GitHub Actions](https://github.com/Page011/Astur/actions), from the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --workspace
```

For changes affecting window management, also **test the behaviour on Windows**, not just compilation. Depending on the change, check Alt-drag move/resize, tiling, workspace switching, multiple monitors, app focus, and quitting Astur cleanly.

If you can't run a particular check or reproduce the issue locally, **say so in the pull request**. Be specific about what you did test; don't claim tests passed if they weren't run.

## Open a pull request

1. Push your branch to your fork.
2. Open a [pull request against `Page011/Astur`](https://github.com/Page011/Astur/compare), normally targeting `main`.
3. Include:
   - **What changed** and why.
   - **Related issue**, for example `Fixes #123` when your PR completely resolves it.
   - **How you tested it**, including any checks you couldn't run.
   - **Screenshots or a short recording** for UI or visual changes, if helpful.
4. Respond to review comments when you're able to. Follow-up commits are fine.

Draft PRs are welcome for work in progress. Please avoid unrelated changes, generated build output, or formatting-only edits to untouched files.

The maintainer works on Astur alongside other commitments. **Review and merge times may vary**, so please don't interpret a delayed response as a lack of interest. Small, self-contained PRs with clear testing are the easiest to review.

## Reporting bugs and suggesting features

Use the [issues](https://github.com/Page011/Astur/issues/new/choose) tab where possible.

For bugs, include the Astur version, Windows version, steps to reproduce, expected versus actual behaviour, and any useful logs or screenshots.

For feature requests, explain the problem you're trying to solve, not just a proposed implementation.

Search existing issues before opening a new one, and add relevant details to an existing report when possible.

## Licensing

By submitting a contribution, you agree that it may be distributed under Astur's existing [Apache License 2.0](LICENSE).

Only submit code and other material you have the right to contribute, and preserve required third-party notices.

---

**The best way to help right now is simple: pick an open issue, work on a fix, and send a PR.** Every contribution helps move Astur forward.
