//! `codexwire`: a client for `OpenAI` Codex's app-server (`codex app-server`,
//! JSON-RPC over the child's stdio), the Codex counterpart to ccwire.
//!
//! Faithful to Codex's own protocol: [`protocol`] is generated from the
//! pinned Codex's JSON Schema (`scripts/gen-codex-protocol`; the schema and
//! its `VERSION` are in `schema/`, [`CODEX_VERSION`] is the version), [`rpc`]
//! frames its messages, [`client`] runs the child and matches responses to
//! requests, and [`auth`] gives a session its own copy of `auth.json`.
//!
//! The README follows (its example is compiled as a doctest).
//!
#![doc = include_str!("../README.md")]
// Owned arguments where the callee keeps them, and Result-returning helpers
// kept uniform; the error docs would only restate the messages.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::unnecessary_wraps
)]
// `#[tokio::test]` expands to `block_on`, which clippy.toml keeps out of
// the library itself.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

extern crate alloc;

pub mod auth;
pub mod client;
#[allow(
    missing_docs,
    clippy::pedantic,
    clippy::large_enum_variant,
    clippy::enum_variant_names,
    clippy::doc_markdown,
    non_camel_case_types,
    reason = "generated"
)]
pub mod protocol;
pub mod rpc;

pub use protocol::CODEX_VERSION;

/// The version in an app-server `userAgent` (`<client>/<version> (...)`).
pub fn user_agent_version(ua: &str) -> Option<&str> {
    let (_, rest) = ua.split_once('/')?;
    rest.split_whitespace().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_from_user_agent() {
        let ua = "my-client/0.158.0 (NixOS 26.11.0; x86_64) dumb (my-client; 0.1.0)";
        assert_eq!(user_agent_version(ua), Some("0.158.0"));
        assert_eq!(user_agent_version("nothing"), None);
    }
}
