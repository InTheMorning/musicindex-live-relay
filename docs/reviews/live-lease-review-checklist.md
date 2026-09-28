# Live Lease Review Checklist

## Scope

Use this checklist after each live lease task packet lands.

Reviewed work:

- `docs/adr/0002-live-lease.md`
- `docs/plans/adr-0002-live-lease-phase-plan.md`
- `docs/tasks/live-lease-task-001-lease-state-and-expiry.md`
- `docs/tasks/live-lease-task-002-keepalive-route-and-docs.md`
- The implementation diff for each packet

## Required Checks

- Only a publish or a keepalive with a correct token renews a lease.
- A listener connection never renews a lease.
- A keepalive never brings back a removed snapshot.
- After an expiry, `remoteValue`, SSE, Socket.IO and the replay buffer serve
  no old snapshot.
- The lease state is in memory only. A restart does not restore it.
- An expiry does not remove the event.
- No lock is held across an emit.
- The wrapped and direct body rule does not change.
- The `remoteValue` body shape does not change.
- Each keepalive status code is in `README.md` and has a test.
- `docs/interoperability.md` records the lease.
- No test sleeps for a lease duration.

## Test Commands

- `cargo fmt -- --check`
- `cargo build`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`

## Review Result

Status: Pass - 2026-09-28. Both packets are merged, and each required check
holds on `master`.

The review changed the lock order in `expire_leases` and `keepalive`.
`expire_leases` checks the lease again under the event write lock, so it never
removes a snapshot that a publish wrote after the first check. The keepalive
renews while it holds the read lock, so an expiry cannot remove the snapshot
between its check and the renewal.
