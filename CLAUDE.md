# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`fzftask` is an fzf-style terminal UI (ratatui + crossterm) that lists tasks from
a `Taskfile.yml`, lets you fuzzy-filter them, and prints the chosen `task <name>`
command to **stdout**. It does not run the task. A zsh wrapper loads that output
onto the shell prompt.

## Commands

```bash
cargo run                 # run the TUI in the current directory's Taskfile
cargo build --release     # release binary at target/release/fzftask
cargo test                # all unit tests (in-module #[cfg(test)])
cargo test <name>         # single test, e.g. cargo test fuzzy_match_is_subsequence
brew style Formula/fzftask.rb   # lint the Homebrew formula
```

There are no integration tests and the TUI cannot be exercised by `cargo test`
(it opens `/dev/tty`). To verify interactive behavior, drive the release binary
under a pseudo-terminal: spawn it with `pty.fork()`, redirect **only fd 1** to a
pipe, send keystrokes to the pty, and read the emitted command from the pipe.

## Architecture

Two source files, one binary.

### `src/main.rs` — UI and app state

- **Output model (the load-bearing design):** the TUI renders to `/dev/tty`, not
  stdout. stdout is reserved for the single selected command. `main()` opens
  `/dev/tty` for both the alternate-screen control codes and the `CrosstermBackend`;
  on Enter it prints `task <name>` to real stdout and exits. This is what lets a
  shell wrapper capture the pick with `$(fzftask)` without UI escape codes leaking
  in. Errors go to stderr so they never pollute the captured command. **A child
  process cannot type into the parent shell** — `shell/fzftask.zsh` defines `ft`,
  which feeds the output to zsh's `print -z` (next-prompt buffer) / a Ctrl-T widget.
- **State machine:** `App` holds `tasks`, the `filtered` index list, the filter
  `input`, a `Focus` (Filter vs Tasks pane), and a `Mode` (`Browse` vs
  `Requires`). All key handling goes through `App::on_key`, which dispatches by
  mode and returns an `Action` (`None` / `Quit` / `Submit(String)`); `run_app`
  turns `Submit` into the stdout print.
- **Filtering** is a case-insensitive fuzzy subsequence match (`fuzzy_match`), so
  `delo` matches `deploy`.
- **`requires` flow:** a task with `requires.vars` enters `Mode::Requires`, which
  collects one value per variable — an `enum` var is a selectable list, a bare var
  is free text — and builds `VAR=val ... task <name>` (values `shell_quote`d).
  `RequiresState::preview_command` renders a live preview as you type/select.

### `src/taskfile.rs` — Taskfile parsing

- `Taskfile::load_from_dir` finds the nearest `Taskfile.yml` and **recursively
  merges `includes`**, namespacing included tasks as `<include>:<task>` (nested →
  `a:b:task`). Cycles are guarded by a visited-set + depth cap; missing/optional
  and remote (`http`) includes are skipped.
- `TaskDef` and several sub-shapes use **custom untagged `Deserialize`** to absorb
  the many forms real Taskfiles take. Be conservative changing these:
  - a task is either a `cmds:` list or a full mapping;
  - a command entry is a string, a `{cmd|task: ...}` map, **or anything else** —
    the `Cmd::Other(IgnoredAny)` catch-all drops unexpected entries (e.g. a `null`
    produced by a comment-only list item `- # note`) instead of failing the whole
    parse;
  - `requires.vars` entries are a bare name or `{name, enum}`;
  - `includes` entries are a path string or `{taskfile, dir, optional}`.
  - Unmodelled task fields (`dir`, `silent`, `status`, `vars`, ...) are ignored.

When a parse change is needed, reproduce against a real-world Taskfile (the
`paak-develop/raund` repo's `taskfile.*.yml` were the source of several edge
cases) and add a regression test that mirrors the construct rather than copying
the file.

## Releasing

Use the **`release` skill** (`.claude/skills/release/SKILL.md`) — it has the full
version-bump → tag → GitHub release → formula-update flow. Key facts:

- The release workflow (`.github/workflows/release.yml`) triggers on a `v*` tag,
  builds with `--locked` (so **`Cargo.lock` must be committed**), and prints the
  source tarball sha256 in the release notes.
- The Homebrew formula lives in **two** places that must stay in sync: this repo's
  `Formula/fzftask.rb` (source of truth) and the tap repo `acxelerator/homebrew-fzftask`
  (local checkout at `../homebrew-fzftask`), which is what `brew tap` reads.
- The formula ships `shell/fzftask.zsh` via `pkgshare.install`; that file must
  exist in the tagged source, and `brew test` asserts it.
