# Architecture

## Chosen mechanism

FrankenFile uses a capability exchange in front of an immutable whole-file content
store. This is the smallest architecture that makes all recipient representations
stable enough for retries and ranges while keeping the six-digit interaction.

```text
operator paths
    │  lstat + O_NOFOLLOW + inode stability check
    ▼
snapshot pipeline ──BLAKE3 identity/SHA-256 HTTP digest──► immutable objects
    │                                                       /var/lib/frankenfile
    └── sorted manifest + keyed code tag ────────────────► SQLite WAL

browser ── six digits ──► redeem gate ── random 256-bit cookie ──► drop manifest
                                                               ├─► object + Range
                                                               └─► deterministic ZIP

Internet ── TLS/H1/H2/H3 ──► reverse proxy ── private bind ──► Rust/Axum
                                  /<base-path>*     127.0.0.1:18766
```

## Trust boundaries

The operator is trusted to select files they are authorized to publish. Selected
source bytes are input, never executable content. The recipient, request headers,
path parameters, codes, cookies, and forwarded client addresses are untrusted.
The configured reverse proxy is the only trusted forwarder, because the
application binds to a private address and accepts forwarding headers only from
the networks named by `--trusted-proxy`. SQLite and the object store share the local disk
durability boundary; neither implies an off-host backup.

## Data model

- `drops`: random public identifier, title, manifest hash, lifetimes, keyed code
  tag, redemption policy, byte/file counts, revoke state.
- `entries`: normalized relative path, kind, immutable BLAKE3 object identity,
  SHA-256 digest, size, media type, and conservative mode metadata.
- `sessions`: keyed token tag, scoped drop, creation/expiry/last-seen/revoke state.
- `redemption_failures`: timestamp and keyed source tag for persistent rolling
  global/per-source budgets. Old rows are pruned transactionally.
- `archives`: manifest/subtree cache identity, build state, size, SHA-256 digest,
  completion time. A stale `building` row can safely be rebuilt after restart.
- `audit_events`: bounded structured events without codes, cookie tokens, or source
  filenames outside the authorized manifest.

All high-entropy identifiers use OS randomness and URL-safe unpadded encoding.
Code generation uses rejection sampling over a 32-bit random value, avoiding
modulo bias. Code/session/source lookup tags use separate BLAKE3 derive-key domains
under a 32-byte local master key.

## Storage and ingestion

Every path component is derived from an operator-supplied root and a `walkdir`
entry with link following disabled. Any symlink or non-regular/non-directory entry
fails the entire publication. Files are opened with `O_NOFOLLOW`; device/inode and
length metadata are compared before and after copying to catch replacement or
mutation. The copy streams once through BLAKE3 and SHA-256, syncs, and moves to a
sharded object path atomically. Existing objects are reused only when length agrees.

The manifest is lexically sorted and inserted in one SQLite transaction only after
every object is durable. Published responses never reopen the original source.

## Archive model

ZIPs are immutable cached representations, not live streams. Their cache key binds
the manifest hash, requested subtree, archive format version, compression policy,
fixed timestamp, and mode policy. Entries are lexical, path separators are `/`,
directories are explicit, timestamps are fixed, and permissions are conservative.
ZIP64 is enabled when needed. One in-process single-flight lock exists per cache key;
SQLite state and atomic rename make crash recovery deterministic.

Because the resulting archive is an ordinary immutable file, the same authorization
and Tower HTTP range service used for content objects supplies HEAD, 206, If-Range,
and 416 behavior.

## HTTP and cache policy

Protected HTML and bytes are `Cache-Control: private, no-store`; a shared proxy must
not retain capability-authorized data. Strong ETags are representation hashes.
`Content-Digest` uses RFC 9530's structured `sha-256=:base64:` form. Download names
have a safe ASCII fallback plus RFC 8187 UTF-8 encoding. User filenames are rendered
as text by Maud and never interpolated into markup, script, or filesystem paths.

Security headers include a script-free, no-third-party CSP; `frame-ancestors 'none'`;
`nosniff`; `Referrer-Policy: same-origin` (no cross-site referrer leakage while
preserving a verifiable same-origin form origin); a restrictive Permissions Policy;
and HSTS at the TLS edge. The interface has no third-party fonts, analytics, or runtime CDN.

## Deferred extensions

FastCDC/Merkle storage is reserved behind the object abstraction. It is adopted
only if an owner-approved representative corpus beats whole-file CAS by at least
1.20x after metadata and exact-restore checks. WebTransport, fountain codes, peers,
and multi-node consensus remain separate regime decisions; the native browser and
single-origin requirements do not currently exhibit the phenomenon they solve.
