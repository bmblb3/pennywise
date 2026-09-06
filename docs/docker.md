# Running Pennywise via Docker

Images are published to `ghcr.io/bmblb3/pennywise` on every `vX.Y.Z` tag, as both the exact version and `latest`.

```sh
docker run -d \
  --name pennywise \
  -p 127.0.0.1:8080:8080 \
  -v pennywise-data:/data \
  ghcr.io/bmblb3/pennywise:latest
```

- The database lives at `/data/pennywise.db` inside the container; mount a volume at `/data` to persist it.
- The container's own `--bind` defaults to `0.0.0.0:8080` (not the binary's own default of `127.0.0.1:8080`) — inside a container's network namespace, binding to `127.0.0.1` would make the server unreachable even with a published port. Reachability is instead gated by whether, and how, you publish the port.
- Per [ADR-0001](adr/0001-no-auth-localhost-only.md), Pennywise has no authentication. Do **not** publish this port to a public or untrusted interface (e.g. `-p 8080:8080` on a host with a public IP) without your own auth layer (reverse proxy, SSH tunnel, Tailscale, WireGuard) in front of it — the example above binds the published port to the host's loopback interface for that reason.
