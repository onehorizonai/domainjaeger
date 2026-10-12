# Releasing

How a new version of domainjaeger reaches people. Pushing a version tag does
all of it: GitHub Actions builds the program for five platforms, puts the
files on a GitHub Release, updates the Homebrew formula and publishes the
crate.

> **Tested on macOS only.** A person has only ever run domainjaeger on macOS
> with Apple Silicon. The Intel Mac, Linux and Windows builds are compiled and
> unit-tested by CI, and that is all. Say so wherever you announce a release.

## What a release delivers

| Channel | What people type | Comes from |
| --- | --- | --- |
| Homebrew | `brew install onehorizonai/tap/domainjaeger` | the formula pushed to `onehorizonai/homebrew-tap` |
| Shell installer (macOS, Linux) | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/onehorizonai/domainjaeger/releases/latest/download/domainjaeger-installer.sh \| sh` | the GitHub Release |
| PowerShell installer (Windows) | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/onehorizonai/domainjaeger/releases/latest/download/domainjaeger-installer.ps1 \| iex"` | the GitHub Release |
| Download | an archive from the [releases page](https://github.com/onehorizonai/domainjaeger/releases), with a `.sha256` next to it | the GitHub Release |
| crates.io | `cargo install domainjaeger` (builds from source: needs Rust and a C compiler) | `cargo publish` |
| cargo-binstall | `cargo binstall domainjaeger` (downloads the archive, builds nothing) | crates.io for the version, the GitHub Release for the file |
| From source | `cargo install --git https://github.com/onehorizonai/domainjaeger` | the repository |

The archives are named after the platform:

| Platform | File |
| --- | --- |
| macOS, Apple Silicon | `domainjaeger-aarch64-apple-darwin.tar.xz` |
| macOS, Intel | `domainjaeger-x86_64-apple-darwin.tar.xz` |
| Linux, x86-64 | `domainjaeger-x86_64-unknown-linux-gnu.tar.xz` |
| Linux, ARM64 | `domainjaeger-aarch64-unknown-linux-gnu.tar.xz` |
| Windows, x86-64 | `domainjaeger-x86_64-pc-windows-msvc.zip` |

Each holds the program, `README.md` and `LICENSE`. The script installers
pick the right one, check its checksum and put the program in
`~/.local/bin` (or `$XDG_BIN_HOME`), adding that folder to the `PATH` when
it is not on it yet.

## Set up once

1. **The repository.** After the first commit:

   ```sh
   gh repo create onehorizonai/domainjaeger --public --source=. --push
   ```

2. **The Homebrew tap.** A second repository that holds the formula. It has
   to exist, with at least one commit, before the first release:

   ```sh
   gh repo create onehorizonai/homebrew-tap --public --add-readme \
     --description "Homebrew formulae"
   ```

3. **`HOMEBREW_TAP_TOKEN`**, so the release may push the formula to the tap.
   Create a fine-grained personal access token on GitHub that can reach only
   `onehorizonai/homebrew-tap`, with *Contents: Read and write*, and store it as a
   secret of this repository:

   ```sh
   gh secret set HOMEBREW_TAP_TOKEN --repo onehorizonai/domainjaeger
   ```

4. **`CARGO_REGISTRY_TOKEN`**, so the release may publish the crate. Create
   a token on <https://crates.io/settings/tokens> with the scopes
   *publish-new* and *publish-update*, and store it the same way:

   ```sh
   gh secret set CARGO_REGISTRY_TOKEN --repo onehorizonai/domainjaeger
   ```

   This one is optional. Without it the release skips crates.io and says so;
   `cargo install domainjaeger` and `cargo binstall domainjaeger` then do
   not work, the other channels do.

| Secret | Needed for | Without it |
| --- | --- | --- |
| `GITHUB_TOKEN` | building and creating the GitHub Release | provided by GitHub, nothing to set up |
| `HOMEBREW_TAP_TOKEN` | pushing `Formula/domainjaeger.rb` to the tap | the Homebrew job fails; the GitHub Release is already up by then |
| `CARGO_REGISTRY_TOKEN` | `cargo publish` | crates.io is skipped |

Not ready for Homebrew yet? Take `"homebrew"` out of `publish-jobs` in
`dist-workspace.toml`, run `dist generate`, and put it back later.

## Every release

1. Set the new version in `Cargo.toml`, run `cargo check` so `Cargo.lock`
   follows, and write down what changed in `CHANGELOG.md`.
2. Check what CI will check:

   ```sh
   cargo fmt --all --check
   cargo clippy --all-targets --locked -- -D warnings
   cargo test --locked
   ```

3. Commit, and wait for CI on `main` to pass.
4. Tag the commit and push the tag. The tag is the version with a `v` in
   front, and it has to match `Cargo.toml`:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

5. Watch it happen: `gh run watch`, or the Actions tab. It takes about ten
   minutes.
6. Try it the way a stranger would, on a Mac:

   ```sh
   brew install onehorizonai/tap/domainjaeger && domainjaeger --version
   ```

A tag with a suffix, like `v0.2.0-rc.1`, makes a prerelease: it gets a
GitHub Release marked as such, and Homebrew and crates.io are left alone.

### What the tag sets in motion

1. **plan**: works out what to build from `dist-workspace.toml`.
2. **build**: one runner per platform builds the program and its archive.
   macOS on `macos-14` and `macos-15-intel`, Linux on `ubuntu-22.04` and
   `ubuntu-22.04-arm`, Windows on `windows-2022`. Nothing is cross-compiled.
3. **installers**: the two install scripts, the Homebrew formula and the
   checksums are generated.
4. **host**: everything is uploaded to a new GitHub Release.
5. **publish**: the formula goes to the tap, the crate to crates.io.

## When it goes wrong

- **One platform fails to build.** Then there is no release at all: a
  release is never half complete. Fix it, move the tag to the fixed commit
  and push again:

  ```sh
  git tag -f v0.1.0 && git push -f origin v0.1.0
  ```

- **The Homebrew job fails.** The GitHub Release is up and the installers
  work. Fix the token or the tap, then run the failed job again from the
  Actions tab.
- **crates.io refuses.** A version can be published once and never again.
  Bump the version and release anew; `cargo yank` hides a broken one.
- **A release has to go.** Delete the GitHub Release and its tag, revert the
  commit in the tap. A crate can only be yanked, not deleted.

## Changing how releases are built

The release workflow, `.github/workflows/release.yml`, is written by
[dist](https://axodotdev.github.io/cargo-dist/) (cargo-dist) from
`dist-workspace.toml`. Do not edit the workflow by hand: change the config
and regenerate it.

```sh
# dist, the version the config names
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v0.33.0/cargo-dist-installer.sh | sh

dist plan        # what a release would hold
dist generate    # rewrite release.yml after a change to dist-workspace.toml
dist build       # build this machine's archive into target/distrib/
dist init        # move to a newer dist
```

On a pull request the release workflow runs `dist plan`, which fails when
the workflow and the config have drifted apart.

`.github/workflows/publish-crates.yml` is ours: the release calls it for the
crates.io step. `.github/workflows/ci.yml` is ours too.

## Good to know

- **Platforms.** See the note at the top. If the Windows or a Linux build
  breaks a release and cannot be fixed quickly, take its line out of
  `targets` in `dist-workspace.toml` and run `dist generate`.
- **Linux.** The binaries are built on Ubuntu 22.04 and need glibc 2.35 or
  newer.
- **Building from source needs a C compiler.** SQLite is compiled into the
  program, and so is the TLS library (`aws-lc`). On macOS that is the Xcode
  command line tools, on Linux `gcc` or `clang`, on Windows the MSVC build
  tools. Neither CMake nor NASM is needed.
- **macOS and Gatekeeper.** The binaries are not signed or notarised. That
  does not matter for Homebrew and the shell installer, which download with
  `curl`. An archive downloaded with a browser is quarantined: macOS refuses
  to run the program until `xattr -d com.apple.quarantine domainjaeger`.
  Signing needs a paid Apple Developer ID.
