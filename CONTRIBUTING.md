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
cargo test --workspace --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
scripts/check-codegen
cargo publish --workspace --dry-run --locked
```

`cargo test` includes README and API doctests. `no_run` examples compile without
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

Configure repository Actions secrets:

- `RELEASE_PLZ_TOKEN`: a GitHub App token or PAT with repository contents,
  pull-request, and workflow write permissions. A user/App token also lets
  release PR updates trigger CI.
- `CARGO_REGISTRY_TOKEN`: a crates.io publishing token for first publication.

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
