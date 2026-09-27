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

Status: Open - 2026-09-27. No packet is complete.
