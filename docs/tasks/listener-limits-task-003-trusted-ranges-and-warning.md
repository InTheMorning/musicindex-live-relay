# Listener Limits Task 003: Trusted Ranges And The Ignored-Header Warning

Status: Implemented - 2026-10-18. It needs tasks 001 and 002.

Every criterion is mechanical.

## Goal

An entry of `TRUSTED_PROXIES` can be a CIDR range. The relay compares the
canonical form of the TCP peer address. The relay logs one warning after
each start when it ignores `X-Real-IP` from a peer that is not trusted.

## Files To Inspect

- `docs/adr/0007-listener-connection-limits.md` (the amendment of
  2026-10-18, §The Client Address, §A Warning For An Ignored Header)
- `src/lib.rs`: `AppConfig::trusted_proxies`, `parse_trusted_proxies`,
  `default_trusted_proxies`, `client_address` and its callers, the tests
  `trusted_proxies_*`
- `tests/api.rs`: the `X-Real-IP` tests of task 001
- `README.md` (the variable table, §The Client Address)

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`

## Do Not Touch

- The limits, their defaults and their results at a limit.
- The `X-Real-IP` rule: one header with one correct IP address.
  `X-Forwarded-For` stays unread.
- The Socket.IO handlers and the SSE handlers, except the argument that
  they give to `client_address`.
- `Cargo.toml`. Use `std` only.
- `docs/adr/**`.

## Constraints

- Add a type for one entry, for example `TrustedProxy { network: IpAddr,
  prefix_len: u8 }`, with a method that tests if it holds an address.
  `AppConfig::trusted_proxies` holds that type.
- An entry without `/` is an address with the full prefix length: 32 for
  IPv4, 128 for IPv6.
- Each of these entries is a configuration error that names
  `TRUSTED_PROXIES`:
  - an address or a prefix length that does not parse,
  - a prefix length above 32 for IPv4 or above 128 for IPv6,
  - a range with bits set after its prefix, for example `172.18.0.1/16`.
- An IPv4 entry never holds an IPv6 address, and an IPv6 entry never holds
  an IPv4 address. The relay first changes the peer with
  `IpAddr::to_canonical`, so `::ffff:127.0.0.1` becomes `127.0.0.1`.
- The client address that `client_address` gives does not change for a
  trusted peer. For a peer that is not trusted, it is the canonical peer
  address.
- The warning: `tracing::warn!` one time for each process, with a
  `static AtomicBool` or `std::sync::Once`. It names the peer address and
  the configured entries. It does not give the value of `X-Real-IP`. A
  request with no `X-Real-IP` causes no warning.
- Put the one-time decision in a function that a unit test can call
  without a log subscriber, for example a function that gives `true` only
  for the first call. Do not test the log output.

## Implementation Steps

1. Add the entry type, its parser and its match method.
2. Change `parse_trusted_proxies` and `default_trusted_proxies` to give the
   entry type.
3. Change `client_address` to use the canonical peer and the entry type,
   and to log the warning.
4. Add the tests of §Acceptance Criteria.
5. In `README.md`, change the `TRUSTED_PROXIES` row and §The Client Address.
   State the CIDR form, the warning, and a container example with a range.

## Acceptance Criteria

Each item is a test:

- With `TRUSTED_PROXIES=127.0.0.1,172.16.0.0/12`, a request from peer
  `172.18.0.1` with `X-Real-IP` counts against the `X-Real-IP` address.
  `tests/api.rs` gives the peer with `MockConnectInfo`.
- With the same value, a request from peer `192.168.1.1` with `X-Real-IP`
  counts against `192.168.1.1`.
- An IPv6 range, for example `fd00::/8`, holds `fd00::1` and does not hold
  `fe80::1`.
- A peer `::ffff:127.0.0.1` with the default value is trusted.
- Each of these values is a configuration error that names
  `TRUSTED_PROXIES`: `10.0.0.0/33`, `::/129`, `172.18.0.1/16`,
  `10.0.0.0/x`, `10.0.0.0/`.
- `0.0.0.0/0` holds each IPv4 address and no IPv6 address.
- The one-time function gives `true` for the first call and `false` for
  each later call.
- The existing `X-Real-IP`, `X-Forwarded-For` and default tests pass with
  no change to their assertions.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- An existing test of the address rule must change its assertion.
- The warning needs a new dependency, or a test needs a log subscriber.
- A caller of `client_address` cannot give the canonical peer.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0007-listener-connection-limits.md
- docs/tasks/listener-limits-task-003-trusted-ranges-and-warning.md
- AGENTS.md
- src/lib.rs, tests/api.rs, README.md

Goal:
- TRUSTED_PROXIES accepts CIDR ranges, the relay compares the canonical peer address, and it logs one warning after each start when it ignores X-Real-IP from a peer that is not trusted.

Constraints:
- Follow §Constraints of the packet exactly. Use std only. Do not test log output. Never log the X-Real-IP value.

Do not touch:
- the limits and their results, the X-Real-IP header rule, X-Forwarded-For, the Socket.IO and SSE handlers except the client_address argument, Cargo.toml, docs/adr/**

Acceptance criteria:
- Each item in §Acceptance Criteria of the packet is a passing test.

Test commands:
- cargo fmt -- --check
- cargo build
- cargo test
- cargo clippy --all-targets -- -D warnings

At the end, report:
1. files changed
2. tests run
3. behavior changed
4. deviations from task
5. unresolved concerns

## Review Result

Reviewed 2026-10-18. The full gate passes with 206 tests. Task 002 ended
with 200.

- The parser, the range match and the canonical peer were correct at the
  first review. The two tests in `tests/api.rs` use the peer `172.18.0.1`
  in `172.16.0.0/12` and the peer `192.168.1.1` out of it.
- `TrustedProxy` is public, because `tests/api.rs` makes an `AppConfig`
  with a range. Its match method stays private.
- The review found three defects and corrected them:
  - The test of the IPv4-mapped peer sent no `X-Real-IP`. The client
    address was then the peer for each result, so the test proved nothing.
    The test now sends `X-Real-IP` and expects that address.
  - The one-time test used the static flag of the relay. Another unit test
    that logs the warning would change its result. The function now takes
    the flag, and the test gives its own flag.
  - `README.md` stated that `172.16.0.0/12` holds each gateway that Docker
    can assign. Docker uses `192.168.0.0/16` when the default pool is full.
    The text now gives the pool, and states that each container in the
    trusted range can set `X-Real-IP`.
