# codexwire

A Rust client for [OpenAI Codex](https://github.com/openai/codex)'s
app-server: it runs `codex app-server` and speaks its JSON-RPC over the
child's stdio, the way Codex's IDE extensions do. The Codex counterpart to
[ccwire](https://github.com/talaagent/ccwire).

Status: early. Checked against Codex 0.158.0 (`codexwire::CODEX_VERSION`).
Unix only (the child's process handling, `auth.json`'s file modes).

- `codexwire::protocol`: serde types for the requests, responses,
  notifications and server requests codexwire uses, generated from the
  app-server's JSON Schema (`codex app-server generate-json-schema`) of the
  pinned Codex. `Request` ties a params type to its method and response.
- `codexwire::rpc`: the JSON-RPC framing (one message a line).
- `codexwire::client`: `spawn_app_server` runs the child with an allowlisted
  environment; `Client` writes requests and matches responses by id, and
  hands the server's notifications and requests over a channel, in order.
  A line that isn't JSON-RPC (or UTF-8) comes out as `Malformed` and the
  connection goes on; once the server's output ends, every call fails
  `Closed`. Calls have no timeout of their own: wrap them in
  `tokio::time::timeout` (a dropped call forgets its request).
  `redact` scrubs secrets out of its stderr.
- `codexwire::auth`: a session's private copy of `auth.json`, copied back
  only if Codex refreshed it and the real one didn't change meanwhile.

From `examples/initialize.rs`:

```rust,no_run
use codexwire::client::{ServerMessage, spawn_app_server};
use codexwire::protocol::{ClientInfo, InitializeParams};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut server = spawn_app_server("codex", &[], &[], std::path::Path::new("."))?;
    let init = server
        .client
        .request(&InitializeParams {
            capabilities: None,
            client_info: ClientInfo {
                name: "example".into(),
                title: None,
                version: "0.1.0".into(),
            },
        })
        .await?;
    println!("codex: {}", init.user_agent);
    server.client.notify("initialized", None).await?;
    // thread/start, turn/start (protocol::{ThreadStartParams, TurnStartParams}),
    // then read what the server sends:
    while let Some(msg) = server.messages.recv().await {
        if let ServerMessage::Notification { method, .. } = msg {
            println!("{method}");
        }
    }
    Ok(())
}
```

## The schema and its pin

`schema/` holds the app-server schema and the Codex version it came from
(`schema/VERSION`); `src/protocol.rs` is generated from it and committed.

- `scripts/gen-codex-protocol`: regenerate `src/protocol.rs` (offline).
- `scripts/gen-codex-protocol --refresh [CODEX]`: re-dump the schema from a
  `codex` binary and move the pin to its version, then regenerate.
- `scripts/gen-codex-protocol --check [CODEX]`: fail unless the binary is
  the pinned version. This checks the binary's version only, not the
  generated code: that is `check-codegen`.
- `scripts/check-codegen`: fail if `src/protocol.rs` isn't what the schema
  makes (CI runs it).

A client can compare the server's version (`user_agent_version` of the
`initialize` response's `userAgent`) to `CODEX_VERSION`.

## License

Apache-2.0 (see LICENSE). The schema is OpenAI Codex's, Apache-2.0 (see
NOTICE).
