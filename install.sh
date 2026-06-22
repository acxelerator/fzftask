#!/bin/bash
# Build and install fzftask from source into ~/.local/bin.
# (Homebrew users should `brew install` instead — see HOMEBREW.md.)
set -e

if ! command -v cargo &>/dev/null; then
  echo "Error: Rust/Cargo is required but not installed." >&2
  echo "Install Rust from https://rustup.rs/" >&2
  exit 1
fi

echo "Building fzftask (release)..."
cargo build --release

INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "${INSTALL_DIR}"
cp target/release/fzftask "${INSTALL_DIR}/"

echo "Installed fzftask to ${INSTALL_DIR}."
echo "Ensure ${INSTALL_DIR} is on your PATH, then source shell/fzftask.zsh and run 'ft'."
