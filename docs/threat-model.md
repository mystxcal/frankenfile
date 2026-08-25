# Threat model

## Assets and adversaries

Assets are unpublished file bytes, manifest names, active drop existence, download
capabilities, the master key, service availability, and integrity of existing host
services. Relevant adversaries are unauthenticated Internet scanners, distributed
guessers, recipients probing outside their authorized drop, malicious filenames and
archives, accidental operator mistakes, source races, and crash/disk-pressure
failures. A privileged host compromise and disclosure by an authorized recipient
are outside the protocol's confidentiality guarantee.

## Required invariants

1. A six-character code authorizes only the redemption endpoint during a short window.
2. Wrong, malformed, expired, exhausted, revoked, and throttled codes do not reveal
   which state occurred.
3. Durable storage and logs contain only keyed lookup tags, never plaintext codes or
   session capabilities.
4. A session is random, scoped to one drop, expiring, revocable, HttpOnly, Secure,
   SameSite, and accepted only below the configured base path.
5. Every served file/archive path is selected from an authorized database row; no
   request path is joined directly to the filesystem.
6. Published bytes are immutable and source-independent.
7. Archive entries cannot be absolute, contain `..`, or escape on extraction.
8. Download headers cannot be split or interpreted as active browser content.
9. Rate-limit state survives application restart; per-source checks supplement a
   low global attempt budget rather than replacing it.
10. Production deployment cannot expose the application port or break any other
    route served by the same edge.

## Controls

| Threat | Control | Residual boundary |
|---|---|---|
| Online enumeration | 15-minute default code life; persistent 10/min global and 5/min source failure budgets; random failure delay; high-entropy exchange | Six characters are still enumerable under a sufficiently distributed, prolonged campaign; operator can shorten life or add an external challenge |
| Code/drop oracle | Keyed constant-size lookup; one failure page/status class; expired/revoked entries excluded by the same query | Network-scale timing is reduced, not mathematically constant |
| Cookie theft/CSRF | Secure HttpOnly scoped cookie, SameSite=Strict, no state-changing recipient GET, no third-party resources, restrictive CSP | A fully compromised recipient browser can use its own capability |
| Path traversal/symlink | Manifest-only retrieval, normalized paths, no link following, `O_NOFOLLOW`, file-type and inode checks | Malicious privileged local races are outside the CLI trust boundary but still detected in ordinary cases |
| Source mutation | Snapshot before publication; size/inode checks; object hash identity | Source may change after successful capture with no effect by design |
| ZIP slip/nondeterminism | Reject unsafe names; lexical entries; fixed metadata; cached immutable representation | Recipients still choose how/where to extract |
| Corrupt object/cache | Hashes at ingest/build; deep doctor rehash; public SHA-256 digest; atomic writes | No background scrubbing or off-host repair in V1 |
| Disk exhaustion | Preflight source totals, archive capacity check, storage reserve, explicit GC, singleflight archive builds | A legitimate large archive may need free space close to its logical size |
| Header/XSS injection | Typed headers, filename sanitization/encoding, Maud escaping, no inline user script | Browser bugs remain external |
| Proxy spoofing | Trust forwarding headers only from the configured `--trusted-proxy` networks; bind only to a private address | Anything already inside that network is within the trust boundary |
| Service compromise | Dedicated user, empty capabilities, systemd filesystem/kernel/device restrictions, no source-tree access | Local object/database bytes are necessarily readable by the service account |
| Deployment regression | Versioned backups, validate-before-reload, health checks before/after, exact rollback command | Existing edge is a shared failure domain |

## Abuse response

Operators can revoke a drop immediately, stop the service without affecting other
routes on the same edge, restore the previous proxy configuration, and inspect
keyed/auditable event counts without recovering attempted codes. Repeated global
throttling is a signal to shorten code lifetimes or add a managed challenge at the
edge; increasing the guess budget is never the availability response.

