# Releasing neonhive

For maintainers. A release is a pull request that prepares the version, followed by a
tag. Pushing the tag starts the `Release` workflow (`.github/workflows/release.yml`),
which tests the tagged commit, creates the GitHub Release from `CHANGELOG.md`, and
publishes to crates.io. Nothing is pushed to `main` by automation.

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

1. `verify tag`: the tag is `vX.Y.Z`, `Cargo.toml` has the same version, and
   `CHANGELOG.md` has a `## [X.Y.Z] - YYYY-MM-DD` heading. Otherwise the run fails with
   an error saying which one is off.
2. `ci`: the same fmt, clippy and test jobs as pull requests, on the tagged commit.
3. `github release`: creates the GitHub Release for the tag, with that version's changelog
   section as the notes. If the release already exists, its notes are updated.
4. `publish to crates.io`: runs `cargo publish` if the `CARGO_REGISTRY_TOKEN` secret is set;
   otherwise it skips with a notice and the run still succeeds. A version that is already
   on crates.io is not an error.

Steps 3 and 4 run in parallel after `ci` passes. Prebuilt binaries are not built yet
(issue #6); a job can be added to `release.yml` that needs `ci`.

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

- `verify tag` fails: the tag points at a commit where `Cargo.toml` or `CHANGELOG.md`
  is wrong. Nothing was published. Fix it through a new pull request, then move the tag:
  `git tag -d vx.y.z && git push origin :refs/tags/vx.y.z`, tag the new merge commit
  and push again. Only do this while no GitHub Release or crates.io version exists for it.
- `ci` fails: same as above (fix by pull request, re-tag), since the tagged commit is
  not releasable.
- `github release` or `publish to crates.io` fails for a transient reason: open the run
  and use "Re-run failed jobs". Both steps are safe to repeat.
- A version was published to crates.io with a mistake: it cannot be overwritten. Run
  `cargo yank --version x.y.z`, fix forward, and release `x.y.(z+1)`.
- The token was missing and the publish was skipped: add the secret, then re-run the
  `Release` workflow for that tag from the Actions tab.
