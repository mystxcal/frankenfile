# Deployment and rollback

## Topology

FrankenFile is a single binary that expects to sit behind a TLS reverse proxy on
a path prefix it owns:

| Piece | Expectation |
|---|---|
| Public origin | Any HTTPS origin you control, e.g. `https://files.example.com` |
| Public route | The configured base path and its descendants, e.g. `/frankenfile` |
| Edge | Any TLS-terminating reverse proxy (Caddy, nginx, Traefik) |
| Upstream | The application bind address, default `127.0.0.1:18766` |
| Service | A dedicated unprivileged system user under systemd |
| State | `/var/lib/frankenfile` (`--data-dir`) |
| Binary | `/usr/local/bin/frankenfile` |

Two settings must agree or share links point at the wrong place: `--base-path`
is the prefix the proxy forwards, and `--public-url` is the externally visible
URL ending in that prefix. The application refuses to start if they disagree.

Bind to loopback or to a private interface only. The application trusts
forwarding headers exclusively from networks passed as `--trusted-proxy`, so
never expose the upstream port publicly, and never widen that flag to `0.0.0.0/0`.

`deploy/` holds a hardened `frankenfile.service`, a `frankenfile-gc.timer` for
the retention sweep, and `Caddyfile.example`. Each is a template: edit the
origin, bind address, and trusted proxy range for your host.

## Operator credential

`--admin-password` / `FRANKENFILE_ADMIN_PASSWORD` gates the FrankenDrop browser
console. No password ships with the source. When it is unset the service
generates one at startup and prints it once to stderr, which is fine for a local
trial but rotates on every restart.

For a real deployment, keep it out of the unit file and out of your shell
history:

```bash
install -d -m 0750 /etc/frankenfile
printf 'FRANKENFILE_ADMIN_PASSWORD=%s\n' "$(openssl rand -base64 18)" \
  > /etc/frankenfile/frankenfile.env
chmod 0640 /etc/frankenfile/frankenfile.env
chown root:frankenfile /etc/frankenfile/frankenfile.env
```

The unit reads it through `EnvironmentFile=`. Rotating the password is a
restart; existing console sessions are cookie-backed and expire on their own
30-minute schedule.

## Install

```bash
cargo build --release
install -m 0755 target/release/frankenfile /usr/local/bin/frankenfile

useradd --system --home /var/lib/frankenfile --shell /usr/sbin/nologin frankenfile
install -d -m 2770 -o frankenfile -g frankenfile /var/lib/frankenfile

install -m 0644 deploy/frankenfile.service /etc/systemd/system/
install -m 0644 deploy/frankenfile-gc.service /etc/systemd/system/
install -m 0644 deploy/frankenfile-gc.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now frankenfile.service frankenfile-gc.timer
```

Then point the proxy at the upstream, preserving the base path, and reload it.
`Caddyfile.example` compresses FrankenFile HTML only: download representations
must pass through untouched so lengths, ranges, ETags, and digests stay
byte-stable.

## Change protocol

1. Build and test a locked release: `cargo fmt --check`, `cargo clippy -D warnings`,
   `cargo test`, `cargo build --release`.
2. Capture SHA-256 of, and copy aside, the current binary, unit files, and proxy
   configuration.
3. Install the new binary, run `frankenfile doctor`, then restart the service.
4. Verify: the service is listening only where you intended, the health endpoint
   answers through the proxy, security headers are present, and a real drop can
   be created, redeemed, and downloaded over the public surface.
5. Keep the pre-change copies until the new revision has been observed healthy.

## Rollback

Rollback never deletes drop state:

1. Restore the previous proxy configuration; validate before reloading.
2. Stop `frankenfile.service` and `frankenfile-gc.timer`.
3. Restore the previous binary and unit files, then `systemctl daemon-reload`.
4. Verify the health endpoint and that the public route behaves as expected.

The state directory survives rollback for inspection or a corrected redeploy.
Deleting objects and the database is a separate, explicit destructive action —
`frankenfile gc --execute` is the only supported reclaim path, and it honours a
retention grace period.

## Backups

Everything durable lives under `--data-dir`: the SQLite database (WAL mode), the
content-addressed object store, cached archives, and the master key. Back the
directory up as a unit with the service stopped, or snapshot the filesystem. The
master key is what makes stored code and session tags meaningful; losing it
invalidates every active drop.
