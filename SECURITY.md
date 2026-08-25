# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately — open a GitHub security
advisory on this repository rather than a public issue. Include the affected
version or commit, what an attacker gains, and the smallest reproduction you
have. Expect an initial response within a few days.

Please do not test against someone else's deployment. Run your own instance.

## Scope

In scope: anything that breaks an invariant in
[`docs/threat-model.md`](docs/threat-model.md) — for example, retrieving a drop
without redeeming a valid code, escaping a drop's scope with a valid session,
recovering plaintext codes or session tokens from storage or logs, path
traversal or ZIP-slip in capture or archive building, response-header injection,
or an oracle that distinguishes a wrong code from an expired one.

Out of scope, by design:

- Anyone who has been given a valid code or a redeemed device can read the drop.
- A privileged compromise of the host, or of the reverse proxy, is outside the
  protocol's confidentiality guarantee.
- Sustained distributed guessing of six-character codes is bounded by rate
  limits and short code lifetimes, not made impossible. Operators can shorten
  code TTLs or add a challenge at the edge.
- The operator is trusted to select files they are authorized to publish.

## Deployment expectations

The service is meant to run behind a TLS reverse proxy, bound to an address the
Internet cannot reach, as an unprivileged user, with `--trusted-proxy` naming
only the proxy's network. Reports that depend on ignoring these are not
vulnerabilities in the software, but corrections to the deployment docs are
welcome.
