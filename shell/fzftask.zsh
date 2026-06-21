# fzftask zsh integration.
#
# Source this from your ~/.zshrc:
#
#     source /path/to/fzftask/shell/fzftask.zsh
#
# Then run `ft` (or press Ctrl-T). Picking a task with Enter pre-fills the next
# prompt with `task <name>` using zsh's `print -z` — it is NOT executed, so you
# can edit it or press Enter yourself to run it.
#
# The binary is resolved in this order:
#   1. $FZFTASK_BIN if set (e.g. export FZFTASK_BIN=~/proj/fzftask/target/release/fzftask)
#   2. `fzftask` on your PATH (install it with `cargo install --path .`)

_fzftask_bin() {
  if [[ -n $FZFTASK_BIN ]]; then
    print -r -- "$FZFTASK_BIN"
  elif command -v fzftask >/dev/null 2>&1; then
    print -r -- fzftask
  else
    return 1
  fi
}

ft() {
  local bin
  if ! bin=$(_fzftask_bin); then
    print -u2 "fzftask: binary not found. Run 'cargo install --path .' or set FZFTASK_BIN."
    return 1
  fi
  local cmd
  cmd=$("$bin" "$@") || return
  [[ -n $cmd ]] && print -z -- "$cmd"
}

# Ctrl-T: open fzftask and insert the command at the cursor on the current line.
ft-widget() {
  local bin cmd
  bin=$(_fzftask_bin) || return
  cmd=$("$bin") || return
  [[ -n $cmd ]] && LBUFFER+="$cmd"
  zle reset-prompt
}
zle -N ft-widget
bindkey '^T' ft-widget
