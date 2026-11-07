# Contributing

Focused bug reports, documentation fixes, protocol fixtures, and pull requests
are welcome. Include your crate versions, Codex CLI version, OS, and a minimal
reproduction. Use synthetic input and remove credentials and personal data.

## Workspace

- `crates/codexwire`: the Unix Tokio client, framing, auth helpers, and exec runs.
- `crates/codexwire-protocol`: generated wire types; only serde dependencies.
- `schema/`: the pinned upstream schema and CLI version.
- `scripts/`: offline code generation and drift checks.

`codexwire::protocol` re-exports the protocol crate. Preserve that path and keep
the protocol crate free of process and async runtime dependencies. Handwritten
Rust belongs outside `generated.rs`.

## Local checks

Use Rust 1.85 or newer; CI's current toolchain is pinned in its workflow.
Python 3, Bash, and rustfmt are needed for code generation. The test suite uses
fake children and synthetic or scrubbed fixtures, with no account or live API.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-features --locked
cargo test --workspace --all-features --doc --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
scripts/check-codegen
cargo publish --workspace --dry-run --locked
```

`cargo test --doc` checks README and API doctests separately from nextest. `no_run` examples compile without
starting a CLI. Run live examples explicitly using your own account. Check the
protocol crate alone with `cargo check -p codexwire-protocol --locked`.
With cargo-deny installed, run `cargo deny check` too.

## Update the Codex pin

Install the intended stable CLI, then run:

```sh
codex --version
scripts/gen-codex-protocol --refresh codex
scripts/check-codegen
```

The refresh updates `schema/VERSION`, the schema bundle, and
`crates/codexwire-protocol/src/generated.rs`. Review all three diffs, generated
Rust API changes, and any generator changes together. Run the full checks above.
Update README compatibility notes and the fake CLI's version when moving the pin.
`--check codex` checks the binary version only; `scripts/check-codegen` compares
regenerated output without modifying the working tree.

Generated enums retain unknown variants as raw JSON. A known variant with an
incompatible body also takes that fallback, so regression tests should assert
that known fixtures decode to known variants. Complex unsupported schema unions
remain `serde_json::Value` rather than pretending to model their shape.

## Fixtures and privacy

`crates/codexwire/tests/corpus` holds an older unauthenticated compatibility
capture. Its version is distinct from the current schema pin. Keep IDs, trace
IDs, timestamps, hostnames, home paths, prompts, and account details synthetic
or scrubbed consistently. Never commit an actual auth.json, token, environment
dump, private transcript, or unsanitized error response. See SECURITY.md for
private reports. Historical contributor attribution remains in Git history.

## Releases

**release-plz** updates release PRs and publishes merged releases. Both crates
share a workspace version and release group. Each has a changelog and a package
prefixed tag (`codexwire-v<version>` and `codexwire-protocol-v<version>`).
This keeps crate releases distinct from older repository tags.

Release PRs use the built-in `GITHUB_TOKEN` by default, with job-level
`contents: write` and `pull-requests: write` permissions. Enable **Allow GitHub
Actions to create and approve pull requests** in the organization and repository
Actions settings, then set repository variable `RELEASE_PR=true`. The setup
check prevents a failing updater from disturbing an existing release PR.

The release workflow dispatches CI explicitly for PRs created with `GITHUB_TOKEN`,
using `actions: write`. It reuses existing runs for the same commit. This uses
GitHub's [workflow dispatch exception](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow)
to the token's event restrictions. An optional `RELEASE_PLZ_TOKEN` (GitHub App
token or PAT) can override the built-in token; no personal token is required.

Configure the `CARGO_REGISTRY_TOKEN` Actions secret for first publication.
It is already configured for this repository.

The release workflow runs only while the repository is public; CI runs while
private too so the first public version can be checked beforehand. Review and
merge a release PR after CI passes. With `release_always = false`, ordinary
pushes do not publish. Release-plz publishes the protocol crate before the
client, then creates tags and GitHub releases.

For the initial release, publish `codexwire-protocol` first, then `codexwire`,
using the configured registry token. Verify both crates.io package pages and
both docs.rs builds before removing the temporary Git-install notes.

For tokenless registry publishing after bootstrap, configure every crate's
trusted publisher: owner `talaagent`, repository `codexwire`, workflow
`release.yml`, environment `release`. Remove the registry secret afterward;
release-plz performs the OIDC exchange itself. A new crate still needs a token
for its first upload.

## License

Contributions use Apache-2.0. Preserve LICENSE and NOTICE in both packages and
retain upstream schema attribution when regenerating types.

Use `jiff` for date/time handling; do not add a direct `chrono` dependency.
The Nix dev shell supplies Rust, Cargo, Clippy, rustfmt, and cargo-nextest.

Run `nix develop` for the pinned toolchain or `nix flake check -L` to run
the workspace through nextest and doctests in the Nix sandbox.


## Trusted publishing

Each crate trusts GitHub owner `talaagent`, repository `codexwire`, workflow
`release.yml`, environment `release`. The publishing job has `id-token: write`;
release-plz performs the OIDC exchange itself. Do not set `CARGO_REGISTRY_TOKEN`
in that job: a persistent token overrides OIDC.

Normal pushes publish only merged release PR versions. For an explicitly
requested initial rollout or recovery, dispatch the release workflow with
`bootstrap=true`; it publishes the committed versions through release-plz.
`release_packages` optionally selects a comma-separated subset, allowing existing
dependencies to publish before a new workspace crate. Empty means the workspace.
The generated configuration is temporary and does not weaken normal push gating.

For a brand-new crate, use a short-lived token with `publish-new` for its first
release, then register the same trusted publisher through the crates.io settings
or its API using `trustpub` permission. Never commit the token. Future updates
use OIDC.
