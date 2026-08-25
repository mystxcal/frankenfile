# FrankenFile production contract

## Outcome

FrankenFile is a browser-native airdrop appliance hosted at exactly one configured
origin and base path, e.g. `https://files.example.com/frankenfile`. An operator can
snapshot one or more existing server files/directories into a time-bounded drop and
receive a six-character rendezvous code. A recipient enters that code, receives a
scoped random session, browses the immutable manifest, and downloads individual
files, folders as ZIPs, or the complete drop as a ZIP.

The six characters are deliberately a rendezvous mechanism, not a long-lived secret.
They expire quickly and are protected by global and per-source attempt budgets.
After redemption, authorization uses a random 256-bit session capability held in
an HttpOnly cookie. No plaintext code or session is stored in SQLite or logs.

## Recipient contract

- The landing page and every recipient action live below the configured base path.
- The only required client is a modern browser; no account, extension, or app.
- Code entry accepts six case-insensitive characters, supports paste, keyboard navigation,
  autofill, reduced motion, high contrast, and narrow/mobile layouts.
- Invalid, expired, exhausted, malformed, and rate-limited redemptions share one
  public response class and one deliberately overlapping timing envelope.
- A successful drop page shows title, expiry, item count, logical size, a short
  integrity fingerprint, and a clear tree/list of included paths.
- Regular files are downloadable individually. Each top-level directory and the
  whole drop are downloadable as deterministic ZIP archives.
- Full file/archive retrieval supports HEAD, standard HTTP byte ranges, strong
  validators, and RFC 9530 SHA-256 `Content-Digest` metadata.
- Source changes after publication cannot alter any recipient-visible byte.
- Expired or revoked drops and sessions cease authorizing new responses.

## Operator contract

The installed `frankenfile` command owns the complete local lifecycle:

```text
frankenfile create <path>... [--title ...] [--code-ttl 15m] [--drop-ttl 24h]
frankenfile get <share-link-or-code> [--output <directory>] [--max-bytes <bytes>]
frankenfile list [--all]
frankenfile show <drop-id>
frankenfile revoke <drop-id>
frankenfile gc [--execute]
frankenfile doctor [--deep]
frankenfile serve ...
```

`create` recursively snapshots regular files and empty directories. It rejects
symlinks, sockets, devices, FIFOs, ambiguous duplicate paths, and input mutation
during capture. It prints the code once, the exact recipient URL, the public drop
identifier, expiry times, and captured byte/file counts. Human output is safe to
paste; `--json` gives stable machine-readable output.

The CLI is safe while the server is running. SQLite serializes metadata changes;
content objects are written to same-filesystem temporary files, synced, and
atomically renamed. A failed publication may leave only unreachable objects, which
`gc` reports before deleting.

## Completion gates

The product is not complete until all of the following are observed:

1. Format, lint, unit, and integration suites pass on the release revision.
2. Short-code exchange, no-plaintext-storage, source immutability, symlink refusal,
   deterministic ZIP, 200/206/416 behavior, expiry, revoke, and GC tests pass.
3. Concurrent redemption/archive requests do not duplicate or corrupt state.
4. Service restart preserves drops, sessions, rate-limit debt, and object bytes.
5. The installed binary and systemd unit run without root privileges and with a
   restricted filesystem, empty capability set, and a host-only bind address.
6. Proxy configuration validates before reload, any co-hosted route on the same
   edge stays healthy after reload, and a captured pre-change file provides rollback.
7. Desktop and mobile screenshots are inspected for overflow, focus, contrast,
   hierarchy, error states, loading behavior, and reduced-motion behavior.
8. A real drop created by the installed CLI is redeemed through the public TLS
   surface; individual and ZIP bytes, ranges, headers, and hashes verify.
9. A requirement-by-requirement audit has no unresolved critical/high finding.
