---
name: release
description: Cut a new fzftask release (version bump → tag → GitHub release → update the Homebrew formula in both the app repo and the homebrew-fzftask tap). Use when asked to "release", "cut vX.Y.Z", "publish a new version", or update the brew formula after changes.
---

# Releasing fzftask

Ship a new version of `fzftask` and update its Homebrew formula.

## Repos & paths

- **App repo** (`acxelerator/fzftask`): the current working directory. Holds the
  code, `Cargo.toml`, `Formula/fzftask.rb`, and `.github/workflows/release.yml`.
- **Tap repo** (`acxelerator/homebrew-fzftask`): local checkout at
  `../homebrew-fzftask` (i.e. `/Users/yoshiki/Development/Projects/homebrew-fzftask`).
  Holds `Formula/fzftask.rb` — this is what `brew tap acxelerator/fzftask` reads.
- The formula lives in **both** repos and must be kept in sync (the app-repo copy
  is the source of truth; copy it to the tap).
- The release workflow triggers on a `v*` tag, builds the binary, creates a
  GitHub release, and prints the **source tarball sha256** in the release notes.

## Pick the version

Use semver. Bug fix → patch (`0.1.4` → `0.1.5`); new feature → minor. Set it once
and reuse it; below it is written as `X.Y.Z` (e.g. `0.1.5`).

## Steps

Run all commands from the app repo root unless noted.

### 1. Bump the version + point the formula at the new tag

- `Cargo.toml`: set `version = "X.Y.Z"`.
- `Formula/fzftask.rb`: set the `url` tag to `vX.Y.Z` and set `sha256 "PENDING_RELEASE"`
  (filled in step 4).

### 2. Build, test, commit, push

```bash
cargo build --release            # also refreshes Cargo.lock
./target/release/fzftask --version   # expect: fzftask X.Y.Z
cargo test                       # all green
git add -A
git commit -m "<summary>; release vX.Y.Z"   # end body with the Co-Authored-By trailer
git push origin main
```

### 3. Tag and run the release workflow

```bash
git tag -a vX.Y.Z -m "fzftask vX.Y.Z"
git push origin vX.Y.Z
# find the run and wait for it
gh run list --repo acxelerator/fzftask --limit 1
gh run watch <run-id> --repo acxelerator/fzftask --exit-status
```

### 4. Get the sha256 and pin it in the formula

```bash
gh release view vX.Y.Z --repo acxelerator/fzftask --json body --jq '.body'
# verify independently — the two must match:
curl -sL https://github.com/acxelerator/fzftask/archive/refs/tags/vX.Y.Z.tar.gz | shasum -a 256
```

Put that hash into `Formula/fzftask.rb` (`sha256 "<hash>"`), then:

```bash
brew style Formula/fzftask.rb    # must report no offenses
git add Formula/fzftask.rb
git commit -m "Pin formula sha256 for vX.Y.Z"
git push origin main
```

### 5. Update the tap repo

```bash
cp Formula/fzftask.rb /Users/yoshiki/Development/Projects/homebrew-fzftask/Formula/fzftask.rb
cd /Users/yoshiki/Development/Projects/homebrew-fzftask
git add Formula/fzftask.rb
git commit -m "Update fzftask formula to vX.Y.Z"
git push origin main
```

### 6. Verify via Homebrew

```bash
brew update
brew upgrade fzftask             # or: brew fetch fzftask to just verify the sha256
fzftask --version                # expect: fzftask X.Y.Z
ls "$(brew --prefix)/opt/fzftask/share/fzftask/fzftask.zsh"   # integration shipped
```

## Notes

- The release workflow uses `--locked`, so **`Cargo.lock` must be committed** with
  the version bump.
- The formula installs `shell/fzftask.zsh` via `pkgshare.install`; that file must
  exist in the tagged source. The `brew test` block asserts it.
- Users on Homebrew 6.0+ must `brew trust acxelerator/fzftask` before installing.
- Commit messages end with the `Co-Authored-By: Claude ...` trailer per repo
  convention. Don't push outside `main` without reason; these pushes are expected
  as part of a release.
