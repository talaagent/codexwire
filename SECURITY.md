# Security policy

Security fixes target the latest published versions. Report vulnerabilities
privately through [GitHub private vulnerability reporting](https://github.com/talaagent/codexwire/security/advisories/new).
If that form is unavailable, email **github@igm.pub**. Include the affected
version, Codex CLI version, platform, and a minimal synthetic reproduction.

Never include real tokens, auth.json contents, private prompts, account IDs,
environment dumps, or unsanitized transcripts in public issues or reports.
Redaction is best effort; raw protocol messages and exec output can be sensitive.

Auth helpers protect copied files with Unix permissions. Applications sharing
credentials across processes must serialize refresh and copy-back operations;
the library's locks coordinate only one process. Report credential handling
problems without including credentials themselves.

Problems in Codex itself should also be reported to the upstream project.
