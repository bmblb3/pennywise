# No authentication; security delegated to a localhost-only bind

Pennywise has no login, no tokens, no auth code anywhere in the codebase. This is safe only because the server binds `127.0.0.1` by default and is never exposed directly to the network — reaching it from another machine means an SSH tunnel, Tailscale, or WireGuard, tools that already solve authenticated remote access well. We considered binding publicly and putting a reverse proxy with basic auth in front, but that still requires the operator to set up and maintain a second service, which conflicts with the single-binary goal. Actually public and unauthenticated was never on the table for a personal ledger.

## Consequences

The `--bind` flag can be pointed at `0.0.0.0` with no error and no warning, at which point anyone who reaches the port has full read/write access to the ledger. If Pennywise ever needs multi-user or direct remote access, this is a real retrofit — auth was never threaded through the request path — not a config change.
