# Changelog

## [Unreleased]

### Added

- Unix Tokio app-server client, isolated exec runs, JSON-RPC framing, and auth helpers.
- Usage examples, package documentation, and release-plz automation.

### Fixed

- Closing the client now drops child stdin so the app-server can exit on EOF.
