# codexwire

[![CI](https://github.com/talaagent/codexwire/actions/workflows/ci.yml/badge.svg)](https://github.com/talaagent/codexwire/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codexwire.svg)](https://crates.io/crates/codexwire)
[![docs.rs](https://docs.rs/codexwire/badge.svg)](https://docs.rs/codexwire)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://github.com/talaagent/codexwire/blob/main/Cargo.toml)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](https://github.com/talaagent/codexwire/blob/main/LICENSE)

A Rust client for [OpenAI Codex](https://github.com/openai/codex). Run
`codex app-server` over stdio for typed requests and streaming notifications,
or use `codex exec --json` for a single prompt in an isolated working directory.

Early release, targeting **Codex 0.167.0**, the stable release used to generate
and check the protocol types. The client supports Unix; the protocol crate is
platform independent. This project is not affiliated with or endorsed by OpenAI.

| Crate | Purpose | Package | API docs |
| --- | --- | --- | --- |
| `codexwire` | Tokio client, JSON-RPC framing, child processes, isolated exec runs, auth helpers | [crates.io](https://crates.io/crates/codexwire) | [rustdoc](https://docs.rs/codexwire) |
| `codexwire-protocol` | Generated serde types, with no process or async runtime dependency | [crates.io](https://crates.io/crates/codexwire-protocol) | [rustdoc](https://docs.rs/codexwire-protocol) |

`codexwire::protocol` re-exports `codexwire-protocol`, so applications can keep
using the client crate's existing import paths.

## Install

Rust 1.85 or newer is required:

```sh
cargo add codexwire
cargo add tokio --features macros,rt,time
```

For wire types alone, use `cargo add codexwire-protocol --git https://github.com/talaagent/codexwire`.
After publication, the equivalent registry commands are `cargo add codexwire`
and `cargo add codexwire-protocol`.

Live examples require the `codex` CLI on PATH. For authenticated work, sign in
with `codex login` first. Parsing protocol messages and running the normal tests
need no login, CLI, or network access.

## Initialize an app-server

Requests use the generated `Request` trait: each params type supplies its method
name and response type. Give calls a timeout; the client does not impose one.

```rust,no_run
use core::time::Duration;
use codexwire::client::spawn_app_server;
use codexwire::protocol::{ClientInfo, InitializeParams};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut server = spawn_app_server("codex", &[], &[], ".".as_ref())?;
    let params = InitializeParams {
        capabilities: None,
        client_info: ClientInfo {
            name: "example".into(), title: None, version: "0.1.0".into(),
        },
    };
    let init = tokio::time::timeout(
        Duration::from_secs(10), server.client.request(&params),
    ).await??;
    println!("codex: {}", init.user_agent);
    server.client.notify("initialized", None).await?;
    server.client.shutdown().await;
    let status = tokio::time::timeout(Duration::from_secs(10), server.child.wait()).await??;
    if !status.success() {
        return Err(std::io::Error::other(format!("app-server exited with {status}")).into());
    }
    Ok(())
}
```

Run the [complete example](https://github.com/talaagent/codexwire/blob/main/crates/codexwire/examples/initialize.rs):

```sh
cargo run -p codexwire --example initialize
```

For conversations, send `ThreadStartParams` and `TurnStartParams`, then consume
`server.messages`. Respond to approval requests explicitly. Notifications and
server requests arrive in order. Malformed JSON or UTF-8 becomes
`ServerMessage::Malformed`; once stdout closes, pending and later calls fail
with `ClientError::Closed`. Dropping the child kills it; for graceful cleanup,
close the client input and wait for the child under a timeout as above.

## Run a single prompt

`Exec` uses a fresh private `CODEX_HOME` and working directory, copies in only
`auth.json`, and copies refreshed credentials back when the original login has
not changed. Its default sandbox is read-only and its timeout is five minutes.
Use an existing ChatGPT login and a private temporary root:

```rust,no_run
use codexwire::exec::{Exec, Input};
use std::path::Path;

async fn query(codex_home: &Path, temporary_root: &Path)
    -> Result<(), Box<dyn core::error::Error>>
{
    let run = Exec::new("codex", codex_home, temporary_root)
        .run(&Input { prompt: "Say hello in one sentence.".into(), images: Vec::new() })
        .await?;
    if let Some(error) = run.failure() {
        return Err(std::io::Error::other(error.to_string()).into());
    }
    if run.code != Some(0) {
        return Err(std::io::Error::other(format!("codex exited with {:?}", run.code)).into());
    }
    println!("{}", String::from_utf8_lossy(&run.last_message));
    Ok(())
}
```

Run [the executable](https://github.com/talaagent/codexwire/blob/main/crates/codexwire/examples/exec.rs)
with `cargo run -p codexwire --example exec -- "Say hello."`. It uses
`CODEX_HOME`, or `$HOME/.codex`, for the existing login. Live runs use your
account. The run's files are removed when `Run` is dropped; copy generated images
out before dropping it if you need to retain them.

## Protocol compatibility

`codexwire-protocol::CODEX_VERSION` records the CLI version used for codegen.
The schema covers the methods and notifications this client currently uses;
raw JSON-RPC calls remain available for other methods. Generated enums keep
unrecognized values as `Unknown(serde_json::Value)` (or `UnknownValue` when the
schema already has an `unknown` variant). A known variant with an incompatible
body also falls back to raw JSON; inspect these values when diagnosing drift.

The client starts the CLI with an allowlisted environment. Additional variables
are explicit. Stderr has best-effort secret redaction; returned events, prompts,
auth files, and raw output can still contain sensitive data. Handle them as
application data rather than assuming all output is safe to log.

The auth refresh lock serializes runs within this process. Applications sharing
one login across processes must also serialize refresh and copy-back operations.
See the [auth API](https://docs.rs/codexwire/latest/codexwire/auth/index.html).

## Development and releases

The workspace keeps runtime code in `crates/codexwire`, generated wire types in
`crates/codexwire-protocol`, and the pinned schema and generators at the root.
See [CONTRIBUTING.md](https://github.com/talaagent/codexwire/blob/main/CONTRIBUTING.md)
for local checks, fixture scrubbing, schema updates, and release-plz setup.
Report bugs in [GitHub Issues](https://github.com/talaagent/codexwire/issues).
Report vulnerabilities privately as described in
[SECURITY.md](https://github.com/talaagent/codexwire/blob/main/SECURITY.md).

## License

[Apache-2.0](https://github.com/talaagent/codexwire/blob/main/LICENSE).
Generated types and the schema derive from OpenAI Codex; attribution is in
[NOTICE](https://github.com/talaagent/codexwire/blob/main/NOTICE).

For development, `nix develop` supplies Rust and cargo-nextest.
See [CONTRIBUTING.md](CONTRIBUTING.md) for tests and protocol generation.
