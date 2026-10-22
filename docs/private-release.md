# Private supercharged releases

The `Private supercharged release` workflow publishes only to the private
`Maciejul/herdr-supercharged` repository. It requires a manual dispatch and an
existing tag matching `vMAJOR.MINOR.PATCH-supercharged.NUMBER`, such as
`v0.9.5-supercharged.1`. Both the workflow job and GitHub API checks require this
repository to remain private. A tag push alone does not trigger this workflow.

## Prepare and publish

1. Set the package version in `Cargo.toml` and its package entry in `Cargo.lock`
   to the intended private version, run the project checks, and commit the release.
2. Create and push an annotated tag for that commit to `origin`. The tag version
   must equal the package version prefixed with `v`.
3. Run the workflow from the default branch, supplying the tag:

   ```sh
   gh workflow run private-release.yml --repo Maciejul/herdr-supercharged --ref master -f tag=v0.9.5-supercharged.2
   ```

4. Inspect the workflow run and the resulting private release. The workflow
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

- `herdr-linux-x86_64`: optimized `x86_64-unknown-linux-gnu` executable.
- `SHA256SUMS`: checksums for the executable and build metadata.
- `build-metadata.json`: repository, tag, commit, target, runner, timestamp, and
  Rust/Zig versions.

This workflow builds on Ubuntu 24.04 with the repository-pinned Rust toolchain
and Zig 0.16.0. The binary requires GNU/glibc; it is not a portable static-musl
build, and older Linux distributions may not satisfy its runtime libraries.
macOS, Windows, ARM, signing, and automatic update distribution are outside this
private workflow. Release downloads require access to the private repository.
The first private release may be built and published locally; its own metadata
records that environment rather than this workflow's Ubuntu runner.

Download with an authenticated GitHub CLI, verify checksums, then install the
executable in a user-controlled location. For example:

```sh
gh release download v0.9.5-supercharged.1 --repo Maciejul/herdr-supercharged --dir herdr-release
cd herdr-release
sha256sum --check SHA256SUMS
install -m755 herdr-linux-x86_64 "$HOME/.local/bin/herdr-supercharged"
```

`herdr-test` remains the separate debug-build launcher. Publishing a private
release does not replace your installed `herdr` or configure its updater.
