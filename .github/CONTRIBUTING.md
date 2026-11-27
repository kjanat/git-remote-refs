# Contributing to git-remote-refs

The Rust workspace implements the Git protocol logic. The npm package uses a
wasm-bindgen build, and the Go module embeds a separately built WebAssembly
component. Changes to Rust code or dependencies can affect all three packages.

## Local development

From the repository root, install the tools pinned in [mise.toml](../mise.toml)
and [rust-toolchain.toml](../rust-toolchain.toml), then install the npm dependencies:

```sh
mise install
rustup toolchain install
mise deps
```

| Command                    | Purpose                                                                                                |
| -------------------------- | ------------------------------------------------------------------------------------------------------ |
| `mise run fmt`             | Format the repository before submitting a PR.                                                          |
| `mise run build`           | Build the Wasm bindings and TypeScript output for npm.                                                 |
| `mise run component:build` | Rebuild the component embedded at `go/git-remote-refs.wasm`.                                           |
| `mise run go:generate`     | Regenerate Go bindings from the [WIT interface](../bindings/component/wit/world.wit).                  |
| `mise run test`            | Build both Wasm targets, type-check TypeScript, run JS and Rust tests, run Go race tests, and lint Go. |

For a shorter feedback loop, use `mise run rstest` for Rust. After building the
npm package, use `mise run jstypes` or `mise run jstest`. For Go, use
`mise run gotest` or `mise run golint`.

## Generated files and CI

After changing Rust code, Cargo dependencies, or the WIT interface, run:

```sh
mise run component:build
mise run go:generate
git diff --stat -- go/
```

Review and commit any resulting changes to the embedded Wasm and tracked Go
bindings alongside the source change. Go consumers compile the bytes committed
in the module. CI rebuilds both Wasm targets, regenerates the bindings, and
checks for a clean diff before testing and packaging. On eligible Renovate PRs,
autofix.ci commits a refreshed component when the binary changes.
[CI workflow](workflows/checks.yml) · [Renovate autofix workflow](workflows/autofix.yml)

## Releases

Maintainers release from the `git-remote-refs` Beepboop stream. Cargo, npm, and
the Go module share a version. Beepboop plans from `master` and owns workflow
dispatch, retries, and signed tags. Use its CLI for releases, including recovery.
The legacy `mise run release` task pushes tags directly and bypasses this process.

1. **Preview the release.** From this checkout, run
   `mise exec -- beepboop release patch --dry-run`. Review the source commit,
   selected packages, and version changes. Use `minor`, `major`, or an explicit
   version in place of `patch` as appropriate.
2. **Start the reviewed release.** Run `mise exec -- beepboop release patch`
   with the same version choice and confirm the preview. If Beepboop opens a
   version PR, review the generated files and CI before merging it.
3. **Check progress.** Run `mise exec -- beepboop runs list`. If a promotion is
   awaiting approval, use
   `mise exec -- beepboop runs publish <run-id> --sourceSha <reviewed-sha>`
   with the commit you reviewed.
4. **Verify publication.** The promotion publishes the crate first. The npm
   operation follows, then the GitHub release operation publishes the refs,
   uploads the crate, npm tarball, and component Wasm to the draft, and publishes
   the release. Check the version on crates.io and npm, the Go module tag
   (`go/vX.Y.Z`), and the assets on the GitHub release (`vX.Y.Z`).

If an operation fails, identify it with `mise exec -- beepboop runs list` and
inspect `mise exec -- beepboop runs recover --help` for recovery options.
A registry upload may have succeeded before a later step failed. Check the
published version before choosing a retry.

[Release configuration](beepboop.json) · [Release workflow](workflows/release.yml)
