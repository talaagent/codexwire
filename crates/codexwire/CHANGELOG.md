# Changelog

## [Unreleased]

## [0.1.2](https://github.com/talaagent/codexwire/compare/codexwire-v0.1.1...codexwire-v0.1.2) - 2026-11-07

### Added

- *(auth)* checkpoint credentials between app-server turns

## [0.1.1](https://github.com/talaagent/codexwire/compare/codexwire-v0.1.0...codexwire-v0.1.1) - 2026-11-07

### Other

- use Jiff and refresh dependencies and Rust development tooling
- link to published crates and record initial releases

## [0.1.0] - 2026-11-07

### Added

- Unix Tokio app-server client, isolated exec runs, JSON-RPC framing, and auth helpers.
- Usage examples, package documentation, and release-plz automation.

### Fixed

- Closing the client now drops child stdin so the app-server can exit on EOF.
