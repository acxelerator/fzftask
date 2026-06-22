# fzftask

An fzf-style terminal user interface for browsing and running [Taskfile](https://taskfile.dev/) tasks, built with [ratatui](https://ratatui.rs/).

Inspired by [acxelerator/taskfile-tui](https://github.com/acxelerator/homebrew-taskfile-tui).

## Install

### Homebrew (macOS/Linux)

```bash
brew tap acxelerator/fzftask
brew trust acxelerator/fzftask   # Homebrew 6.0+ requires trusting third-party taps
brew install fzftask

# Enable the zsh integration so picks load onto your prompt (see Shell integration)
echo 'source $(brew --prefix)/opt/fzftask/share/fzftask/fzftask.zsh' >> ~/.zshrc
```

See [HOMEBREW.md](HOMEBREW.md) for tap and release details.

### From source

```bash
cargo install --path .   # installs the `fzftask` binary
# or just run it in place:
cargo run
```

## Keybindings

| Key         | Focus  | Action                              |
| ----------- | ------ | ----------------------------------- |
| `Tab`       | any    | Switch focus (filter ↔ tasks)       |
| `Esc`       | any    | Quit without selecting              |
| `Enter`     | any    | Select task, close UI, emit command |
| _any char_  | filter | Type into the filter box            |
| `Backspace` | filter | Delete a character from filter      |
| `↑` / `↓`   | both   | Move selection                      |

The filter box does a case-insensitive fuzzy (subsequence) match on task
names, so `delo` matches `deploy`.

### Required variables

If a task declares `requires.vars`, pressing `Enter` opens a prompt to collect
each variable before the command is emitted:

- a variable with an `enum` is chosen from its candidate values (`↑`/`↓`, `Enter`);
- a variable without an `enum` is typed in (`Enter` to confirm);
- `Esc` cancels and returns to the task list.

The result is a command like `NAME=web ENV=prod task deploy` (values that need
it are shell-quoted). For example:

```yaml
tasks:
  deploy:
    requires:
      vars:
        - NAME                       # free-form input
        - name: ENV                  # pick from a list
          enum: [dev, staging, prod]
```

### Includes

Tasks pulled in via `includes` are listed with the include's namespace prefix
(e.g. `docs:serve`), just like `task` itself. Nested includes and directory
includes are followed; `optional: true` includes that are missing are skipped,
and remote (`http(s)`) includes are ignored.

```yaml
includes:
  docs: ./taskfiles/docs.yml   # adds docs:serve, docs:build, …
```

## Shell integration

fzftask renders to `/dev/tty` and prints the chosen `task <name>` to stdout, so
a shell wrapper can drop the command onto your prompt. Pressing `Enter` closes
the UI and hands the command to the shell **without running it** — you can edit
it or press Enter yourself.

> **Note:** A bare `fzftask` cannot type into your prompt by itself — no child
> process can. The prompt is filled by the **zsh wrapper** below, which captures
> the binary's output and feeds it to the line editor. Running `fzftask`
> directly just prints the command.

For zsh:

```zsh
# 1. Install the binary so the wrapper can find it
cargo install --path .          # installs `fzftask` into your cargo bin
#    (or, without installing, point the wrapper at a build:)
#    export FZFTASK_BIN=/path/to/fzftask/target/release/fzftask

# 2. Source the integration in ~/.zshrc
source /path/to/fzftask/shell/fzftask.zsh
```

Then run `ft` to pick a task (it uses `print -z` to pre-fill the next prompt),
or press `Ctrl-T` to insert the command at the cursor. If `ft` prints
`binary not found`, `fzftask` isn't on your `PATH` — install it or set
`FZFTASK_BIN`.

## Status

Early scaffold. fzftask reads tasks from a `Taskfile.yml` (or `Taskfile.yaml`)
in the current directory, shows each task's description/summary/commands, lets
you filter by name, and emits the selected `task <name>` to the shell on Enter.
It does **not** execute the task itself.

## License

MIT
