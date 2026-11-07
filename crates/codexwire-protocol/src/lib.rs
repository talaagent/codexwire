//! Generated wire types for the OpenAI Codex app-server.
//!
//! This crate needs only serde and `serde_json`. Use it to parse or construct
//! protocol messages without spawning a process or depending on Tokio.
//!
#![doc = include_str!("../README.md")]
#![deny(rustdoc::broken_intra_doc_links)]

extern crate alloc;

#[allow(
    missing_docs,
    clippy::pedantic,
    clippy::large_enum_variant,
    clippy::enum_variant_names,
    clippy::doc_markdown,
    non_camel_case_types,
    reason = "generated from the pinned upstream JSON Schema"
)]
mod generated;

pub use generated::*;
