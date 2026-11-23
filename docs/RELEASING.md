# Releasing neonhive

For maintainers. A release is a pull request that prepares the version, followed by a
tag. Pushing the tag starts the `Release` workflow (`.github/workflows/release.yml`),
which tests the tagged commit, builds prebuilt binaries, creates the GitHub Release from
`CHANGELOG.md` with the binaries attached, publishes to crates.io and updates the Homebrew
tap. Nothing is pushed to `main` by automation.

## Steps

Replace `x.y.z` with the new version (Semantic Versioning).

1. Create the branch from an up-to-date `main`:

   ```sh
   git checkout main && git pull
   git checkout -b chore/release-x.y.z
   ```

2. Bump `version` in the `[package]` table of `Cargo.toml`, then refresh the lock file
   so it records the new version:

   ```sh
   cargo check
   git diff Cargo.lock    # only the neonhive version line changes
   ```

3. In `CHANGELOG.md`, move everything under `## [Unreleased]` below a new heading
   `## [x.y.z] - YYYY-MM-DD` (today's date), leaving `## [Unreleased]` empty. Update the
   link definitions at the bottom:

   ```
   [Unreleased]: https://github.com/juanpalopez/neonhive/compare/vx.y.z...HEAD
   [x.y.z]: https://github.com/juanpalopez/neonhive/releases/tag/vx.y.z
   ```

4. Check locally:

   ```sh
   scripts/check-release-version.sh vx.y.z
   scripts/changelog-section.sh x.y.z      # prints the notes the release will carry
   cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
   ```

5. Commit and open a pull request titled `chore: release x.y.z`. Merge it (squash) once
   the required checks pass.

6. Tag the merge commit on `main` and push the tag:

   ```sh
   git checkout main && git pull
   git tag -a vx.y.z -m "vx.y.z"
   git push origin vx.y.z
   ```

   A tag with a suffix, such as `v1.0.0-rc.1`, is published as a GitHub pre-release. The
   `Cargo.toml` version and the changelog heading must use the same suffix.

## What the workflow does

1. `verify tag`: the tagged commit is on `main` (an ancestor of, or equal to,
   `origin/main`; a tag on any other branch never publishes), the tag is `vX.Y.Z`,
   `Cargo.toml` has the same version, and `CHANGELOG.md` has a `## [X.Y.Z] - YYYY-MM-DD`
   heading. Otherwise the run fails with an error saying which one is off.
2. `ci`: the same fmt, clippy and test jobs as pull requests, on the tagged commit.
3. `build`: one job per target builds the release binary with `cargo build --release
   --locked` and packages it with `scripts/build-release-archive.sh` into
   `neonhive-vX.Y.Z-<target>.tar.gz` (the binary, `LICENSE`, `README.md`) plus a
   `.sha256` file. Targets: `aarch64-apple-darwin`, `x86_64-apple-darwin` (cross-compiled
   on Apple silicon), `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` (Ubuntu
   22.04, for an older glibc), `x86_64-unknown-linux-musl` (static). Every binary except
   `x86_64-apple-darwin` is run once (`--version`, a `--demo --snapshot` frame) before it
   is kept. The build jobs can only read the repository.
4. `github release`: once every target has built, creates the GitHub Release for the
   tag with that version's changelog section as the notes (if it already exists, its
   notes are updated), checks each archive against its `.sha256`, and uploads the
   archives, the `.sha256` files and a combined `SHA256SUMS`. Assets of an earlier
   attempt are replaced.
5. `publish to crates.io`: runs `cargo publish` if the `CARGO_REGISTRY_TOKEN` secret is set;
   otherwise it skips with a notice and the run still succeeds. A version that is already
   on crates.io is not an error.
6. `update homebrew tap`: if the `HOMEBREW_TAP_TOKEN` secret is set, renders the formula
   for the tag with `scripts/render-homebrew-formula.sh` and pushes it as
   `Formula/neonhive.rb` to `juanpalopez/homebrew-tap`. Otherwise, and for pre-releases,
   it skips with a notice and the run still succeeds. An unchanged formula is not
   committed again.

The steps run one after another, each only when the one before succeeded. crates.io comes
after the binaries and the release on purpose: a crates.io version cannot be replaced, so
anything that could fail in a way that needs a code fix and a new tag runs first.

## Homebrew tap

The formula is kept in this repository at `packaging/homebrew/neonhive.rb`; the tap holds
a copy with the `url` and `sha256` of the latest release. It builds from the GitHub
source tarball of the tag, with Homebrew's own Rust as a build dependency, so users do not
need Rust installed. Edit the formula here, never only in the tap: the next release
overwrites the tap's copy.

homebrew-core is not an option yet: it requires, for a project submitted by its own
author, at least 225 stars, 90 forks or 90 watchers (see
[Acceptable Formulae](https://docs.brew.sh/Acceptable-Formulae) and the
[Package Acceptance Policy](https://docs.brew.sh/Package-Acceptance-Policy)).

One-time setup:

1. Create a public repository `juanpalopez/homebrew-tap` (the `homebrew-` prefix is what
   makes `brew install juanpalopez/tap/neonhive` work). An initial README is enough; the
   workflow creates `Formula/`. Optionally run `brew tap-new juanpalopez/homebrew-tap`
   locally first and push it, which adds Homebrew's bottle-building workflows; they are
   not needed for this formula.
2. Create a fine-grained personal access token: Settings, Developer settings, Personal
   access tokens, Fine-grained tokens. Resource owner `juanpalopez`; repository access
   "Only select repositories", `juanpalopez/homebrew-tap` only; repository permissions
   Contents: Read and write (Metadata: Read-only is added automatically); nothing else.
   Give it an expiry and note the date.
3. In this repository, Settings, Secrets and variables, Actions, add a repository secret
   named `HOMEBREW_TAP_TOKEN` with that token.

Risk: anyone who can run a workflow with access to that secret can push any formula to
the tap, and a formula runs code on every machine that installs it. The secret is only
read by the `Release` workflow, which runs on tags of commits on `main`; keep tag and
`main` protection as they are and scope the token to the tap repository only. To publish
the tap without a token, leave the secret unset and update the tap by hand:

```sh
scripts/render-homebrew-formula.sh x.y.z > ../homebrew-tap/Formula/neonhive.rb
cd ../homebrew-tap && git commit -am "neonhive x.y.z" && git push
```

`scripts/update-homebrew-tap.sh x.y.z` does the same from a temporary clone;
`DRY_RUN=1` commits there without pushing.

To check a formula change before a release, copy it into a local tap and let Homebrew
build and test it:

```sh
brew tap-new --no-git local/neonhive-test
cp packaging/homebrew/neonhive.rb "$(brew --repository)/Library/Taps/local/homebrew-neonhive-test/Formula/"
brew trust --formula local/neonhive-test/neonhive
brew audit --strict --online --formula local/neonhive-test/neonhive
brew install --build-from-source local/neonhive-test/neonhive
brew test local/neonhive-test/neonhive
brew uninstall neonhive && brew untrust --formula local/neonhive-test/neonhive && brew untap local/neonhive-test
```

## crates.io token

1. On crates.io, open Account Settings, API Tokens, and create a token. Scope it to the
   `publish-update` (and `publish-new`, for a first publish) endpoint and to the `neonhive`
   crate if it already exists.
2. In the GitHub repository, Settings, Secrets and variables, Actions, add a repository
   secret named `CARGO_REGISTRY_TOKEN` with that value.

crates.io also supports trusted publishing, which exchanges a GitHub OIDC identity for a
short-lived token and removes the long-lived secret. It is not set up here; see the
crates.io documentation if you want to switch.

## If a release fails

- `verify tag` fails with "is not on main": the tag points at a commit that is not in
  `main`, for example a branch commit. Nothing was published. Delete the tag
  (`git tag -d vx.y.z && git push origin :refs/tags/vx.y.z`), merge the release pull
  request, then tag the merge commit on an up-to-date `main` as in step 6. The same
  check fails if `origin main` could not be fetched; re-run the workflow in that case.
- `verify tag` fails on the version or changelog: the tag points at a commit where `Cargo.toml` or `CHANGELOG.md`
  is wrong. Nothing was published. Fix it through a new pull request, then move the tag:
  `git tag -d vx.y.z && git push origin :refs/tags/vx.y.z`, tag the new merge commit
  and push again. Only do this while no GitHub Release or crates.io version exists for it.
- `ci` fails: same as above (fix by pull request, re-tag), since the tagged commit is
  not releasable.
- `build`, `github release`, `publish to crates.io` or `update homebrew tap` fails for a
  transient reason (a runner or network problem): open the run and use "Re-run failed
  jobs". Every step is safe to repeat; the later steps run again after it.
- A `build` job fails for a reason that needs a code fix: nothing was published, because
  the release, crates.io and the tap wait for every build. Fix it through a new pull
  request and move the tag, as for a failed `ci`. If a GitHub Release already exists from
  an earlier attempt, delete it first (`gh release delete vx.y.z`).
- `github release` succeeded but `publish to crates.io` failed for a reason that needs a
  fix: the release can still be deleted and the tag moved, as long as nothing reached
  crates.io.
- A version was published to crates.io with a mistake: it cannot be overwritten. Run
  `cargo yank --version x.y.z`, fix forward, and release `x.y.(z+1)`.
- The token was missing and the publish was skipped: add the secret, then re-run the
  `Release` workflow for that tag from the Actions tab. The same holds for the tap and
  `HOMEBREW_TAP_TOKEN`, or update the tap by hand as shown above.
