# codexwire

A Rust client for [OpenAI Codex](https://github.com/openai/codex)'s
app-server: it runs `codex app-server` and speaks its JSON-RPC over the
child's stdio, with serde types generated from the app-server's JSON Schema
and pinned to one Codex version. The Codex counterpart to
[ccwire](https://github.com/talaagent/ccwire).

Status: scaffold. The crate lands in the first PR.

## License

Apache-2.0 (see LICENSE).
