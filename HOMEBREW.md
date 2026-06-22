# Homebrew distribution

fzftask ships a Homebrew formula in [`Formula/fzftask.rb`](Formula/fzftask.rb)
that builds from source (`cargo install`), so it works on macOS and Linux with
only the `rust` build dependency.

## For users

```bash
# Tap straight from this repository (works without a dedicated tap repo)
brew tap acxelerator/fzftask https://github.com/acxelerator/fzftask
brew install fzftask
```

If you publish a dedicated tap repository named `homebrew-fzftask`, users can
instead run the shorter:

```bash
brew tap acxelerator/fzftask
brew install fzftask
```

After installing, enable the shell integration (see the README):

```zsh
source "$(brew --prefix)/share/fzftask/fzftask.zsh"   # if you ship it as a resource
# or point at the repo copy:
source /path/to/fzftask/shell/fzftask.zsh
```

## For the maintainer — cutting a release

1. **Tag and push.** The [`Release`](.github/workflows/release.yml) workflow
   triggers on tags matching `v*`:

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

   It builds the binary, creates a GitHub release, and prints the source
   tarball's `sha256` in the release notes.

2. **Update the formula.** Put that hash into `Formula/fzftask.rb` (and bump the
   `url`/version for later releases):

   ```bash
   curl -sL https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.0.tar.gz | shasum -a 256
   ```

   ```ruby
   url "https://github.com/acxelerator/fzftask/archive/refs/tags/v0.1.0.tar.gz"
   sha256 "<hash from above>"
   ```

3. **Commit the formula** so `brew tap`/`brew install` resolve the new version.

## Testing the formula locally

```bash
brew install --build-from-source ./Formula/fzftask.rb
fzftask --version
brew test fzftask
brew uninstall fzftask
```

The formula's `test do` block runs `fzftask --version`, which exits without
opening a terminal — safe to run in CI / `brew test`.
