# Supercharged releases

The `Supercharged release` workflow publishes only to the
`maciejul/herdr-supercharged` repository. It requires a manual dispatch and an
existing tag matching `vMAJOR.MINOR.PATCH-supercharged.NUMBER`, such as
`v0.9.5-supercharged.1`. The workflow job checks the repository name. A tag push alone does not trigger this workflow.

## Prepare and publish

1. Set the package version in `Cargo.toml` and its package entry in `Cargo.lock`
   to the intended version, run the project checks, and commit the release.
2. Create and push an annotated tag for that commit to `origin`. The tag version
   must equal the package version prefixed with `v`.
3. Run the workflow from the default branch, supplying the tag:

   ```sh
   gh workflow run supercharged-release.yml --repo maciejul/herdr-supercharged --ref master -f tag=v0.9.5-supercharged.7
   ```

4. Inspect the workflow run and the resulting release. The workflow
   builds the tagged source, smoke-tests `herdr --version`, uploads a draft,
   downloads its assets to verify their checksums, then publishes it. It refuses
   to overwrite an existing release. If publication fails after draft creation,
   inspect and remove only that failed draft before retrying; preserve the tag.

The workflow uses the repository-scoped `GITHUB_TOKEN` with `contents: write`;
no extra secrets, upstream credentials, public release services, or deployment
keys are needed. Checkout credentials are not persisted. Source dependencies
and pinned build-tool actions are downloaded normally. The workflow does not
run the full test suite: run it before tagging. It does not publish to
`herdrdev/herdr`, change upstream update manifests, or mark the release latest.
Keep inherited upstream publishing workflows disabled unless separately audited.

## Assets and compatibility

Supercharged builds include the MIT-licensed `ntindle/herdr-resurrect` add-on inside
the executable. A fresh server installs it locally without GitHub access; Node.js
16+ must be on PATH, with bash/fzf additionally needed for saved-space overlays.
There are no npm dependencies. Existing explicit plugin installations are kept,
and a disabled bundled plugin stays disabled across upgrades. User settings and
snapshots stay in the normal plugin directories. Auto-restore defaults to off.

The server takes a full-session Resurrect snapshot every hour, without a separate
autosave tab. Snapshot archives older than 30 days are removed on saves, including
archives of stopped named sessions; named saved spaces are retained. Suspension
still uses native exact-session persistence and does not wait on plugin snapshots.

Suspended tabs retain exact Claude/Codex session IDs through shutdown hooks and
native save/restore. A native pending resume plan can recover reporting metadata
lost by older code; directory-wide "continue" commands never override a suspended
tab's exact conversation. Manual resume remains available when automatic startup
resume is disabled. The add-on leaves suspended tabs to native Herdr recovery.
See `vendor/herdr-resurrect/SUPERCHARGED-COMPATIBILITY.md` for provenance and limitations.

Validate the distribution with `just bundled-resurrect-test`; it checks a fresh
offline install and real plugin save/preview/restore against a disposable server.

- `herdr-linux-x86_64`: optimized `x86_64-unknown-linux-gnu` executable.
- `SHA256SUMS`: checksums for the executable and build metadata.
- `build-metadata.json`: repository, tag, commit, target, runner, timestamp, and
  Rust/Zig versions.

This workflow builds on Ubuntu 24.04 with the repository-pinned Rust toolchain
and Zig 0.16.0. The binary requires GNU/glibc; it is not a portable static-musl
build, and older Linux distributions may not satisfy its runtime libraries.
macOS, Windows, ARM, signing, and automatic update distribution are outside this
workflow. Release downloads use the public repository.
The first release may be built and published locally; its own metadata
records that environment rather than this workflow's Ubuntu runner.

Download with an authenticated GitHub CLI, verify checksums, then install the
executable in a user-controlled location. For example:

To replace your normal `~/.local/bin/herdr` installation on Linux x86_64/WSL:

```sh
./scripts/install-supercharged-release.sh
# Or select a future release explicitly:
./scripts/install-supercharged-release.sh v0.9.5-supercharged.7
```

The script defaults to `v0.9.5-supercharged.7`, checks the platform and glibc,
downloads through `gh`, verifies checksums and the executable's version, then
backs up the existing binary and atomically replaces it. It refuses to overwrite
symlinks. Running servers, configuration, state, and integrations are untouched.
Restart servers when active work is finished to load server-side changes;
stopping a server ends its live pane processes. The inherited `herdr update`
command still targets upstream, so use this script for supercharged upgrades.

## Run alongside normal Herdr as `herdr-supercharged`

Use this setup to try the optimized supercharged release without replacing
`~/.local/bin/herdr`. Run the setup from an external WSL/Linux terminal with
authenticated `gh` access. The release requires Linux x86_64 and glibc 2.39+.
The existing `herdr-test` command remains the source/debug-build launcher.

Download and verify the release, then install it under a separate name:

```bash
(
  set -euo pipefail
  release_dir=$(mktemp -d)
  trap 'rm -rf -- "$release_dir"' EXIT
  gh release download v0.9.5-supercharged.7 \
    --repo maciejul/herdr-supercharged --dir "$release_dir" \
    --pattern herdr-linux-x86_64 --pattern SHA256SUMS \
    --pattern build-metadata.json
  cd "$release_dir"
  sha256sum --check --strict SHA256SUMS
  chmod +x herdr-linux-x86_64
  ./herdr-linux-x86_64 --version
  mkdir -p "$HOME/.local/bin"
  install -m755 herdr-linux-x86_64 "$HOME/.local/bin/herdr-supercharged.new"
  mv -f "$HOME/.local/bin/herdr-supercharged.new" "$HOME/.local/bin/herdr-supercharged"
)
```

A different executable name alone does **not** isolate a release build: plain
`herdr-supercharged` would still use normal Herdr's configuration and default session.
Launch a dedicated named session instead:

```bash
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
  "$HOME/.local/bin/herdr-supercharged" --session herdr-supercharged
```

This separates running panes and session layout, but still shares normal Herdr's
global configuration. Always launch from an external
terminal to avoid nested-Herdr restrictions.

For isolated configuration and state as well, add this function to `~/.zshrc`
(or `~/.bashrc` for Bash), then open a new terminal:

```bash
herdr-supercharged() {
  env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_SESSION \
    -u HERDR_CONFIG_PATH \
    XDG_CONFIG_HOME="$HOME/.config/herdr-supercharged-profile" \
    XDG_STATE_HOME="$HOME/.local/state/herdr-supercharged-profile" \
    "$HOME/.local/bin/herdr-supercharged" --session herdr-supercharged "$@"
}
```

With that function loaded:

```bash
herdr-supercharged --version
herdr-supercharged                 # Launch or reconnect to the supercharged session.
herdr-supercharged server stop     # Only when its active work is finished.
```

The isolated configuration is at
`~/.config/herdr-supercharged-profile/herdr/config.toml`; state is under
`~/.local/state/herdr-supercharged-profile/herdr/`. This starts with fresh preferences;
it does not copy normal Herdr or debug `herdr-test` settings. XDG overrides are
also inherited by programs launched inside its panes, so those programs may
use separate XDG settings too. Agent integration installation can still modify
shared agent hook files; this profile is not a sandbox for integration changes.

Repeat the download block with a new tag to update the separate binary. Then
stop and relaunch the supercharged session when ready; stopping ends its live pane
processes. Do not run `herdr-supercharged update`: its updater still targets upstream.
The replacement installer above targets `~/.local/bin/herdr`, so use this separate
download procedure when maintaining `herdr-supercharged`.

Publishing a release does not replace either installed binary or
configure its updater.
