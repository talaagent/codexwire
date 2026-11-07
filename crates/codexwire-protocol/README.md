# codexwire-protocol

[![crates.io](https://img.shields.io/crates/v/codexwire-protocol.svg)](https://crates.io/crates/codexwire-protocol)
[![docs.rs](https://docs.rs/codexwire-protocol/badge.svg)](https://docs.rs/codexwire-protocol)
[![CI](https://github.com/talaagent/codexwire/actions/workflows/ci.yml/badge.svg)](https://github.com/talaagent/codexwire/actions/workflows/ci.yml)

Generated serde types for **Codex 0.167.0**'s app-server JSON-RPC protocol.
This crate depends on serde and `serde_json`, with no Tokio, I/O, or child process
support. For a client, use [codexwire](https://docs.rs/codexwire).

Initial crates.io publication is pending. Install from Git for now:

```sh
cargo add codexwire-protocol --git https://github.com/talaagent/codexwire
cargo add serde_json
```

## Construct a typed request

```
use codexwire_protocol::{ClientInfo, InitializeParams, Request};

let params = InitializeParams {
    capabilities: None,
    client_info: ClientInfo {
        name: "example".into(), title: None, version: "0.1.0".into(),
    },
};
assert_eq!(InitializeParams::METHOD, "initialize");
let json = serde_json::to_value(&params)?;
assert_eq!(json["clientInfo"]["name"], "example");
# Ok::<(), serde_json::Error>(())
```

`Request` ties params to a method and response type. `Notification` and
`ServerRequest` identify the server messages supported by the client. These
are message bodies; a JSON-RPC transport adds IDs, method names, and envelopes.

## Read unfamiliar variants

```
use codexwire_protocol::ThreadItem;
use serde_json::json;

let raw = json!({"type": "futureItem", "id": "example", "newField": true});
let item: ThreadItem = serde_json::from_value(raw.clone())?;
assert!(matches!(&item, ThreadItem::Unknown(_)));
assert_eq!(serde_json::to_value(item)?, raw);
# Ok::<(), serde_json::Error>(())
```

A known tag whose body fails to decode also lands in the raw fallback. Not every
schema method is generated, and some complex bodies deliberately remain
`serde_json::Value`. The input schema is committed with a version pin; regenerate
with `scripts/gen-codex-protocol` from the repository root. Packaged consumers
need neither the schema, Python, nor a Codex binary to build this crate.

`CODEX_VERSION` records the upstream CLI pin. Package versions follow the public
Rust API's compatibility, independently of the Codex CLI version.

See [CONTRIBUTING.md](https://github.com/talaagent/codexwire/blob/main/CONTRIBUTING.md)
for schema updates and release instructions. Licensed under
[Apache-2.0](https://github.com/talaagent/codexwire/blob/main/LICENSE), with upstream
attribution in [NOTICE](https://github.com/talaagent/codexwire/blob/main/NOTICE).
