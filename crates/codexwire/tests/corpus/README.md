# Compatibility fixtures

`codex-0.158.0-turn-unauthenticated.jsonl` is an older unauthenticated
app-server capture: initialize, thread/start, and a turn rejected without a
login. It tests backward decoding with the current generated types; it does
not claim a successful authenticated turn on that older CLI.

Prompts are a trivial greeting. Home/work paths and hostnames are placeholders;
UUIDs, timestamps, OS identity, request IDs, and Cloudflare trace metadata are
synthetic. Relative event timing is retained. Before adding fixtures, normalize
these fields consistently and remove all account data and credentials.
