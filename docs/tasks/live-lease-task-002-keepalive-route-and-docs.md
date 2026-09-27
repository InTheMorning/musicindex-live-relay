# Live Lease Task 002: Keepalive Route And Documents

Status: Ready - 2026-09-27. It needs task 001.

Every criterion is mechanical. This service has no user interface, so it has
no visual criteria.

## Goal

Add `POST /v1/liveitems/{event_id}/keepalive`. Add the lease fields to
`GET /v1/liveitems/{event_id}/metadata`. Record the lease contract in
`README.md` and `docs/interoperability.md`.

## Files To Inspect

- `docs/adr/0002-live-lease.md` (§The Keepalive Route and §Wire Changes)
- `docs/tasks/live-lease-task-001-lease-state-and-expiry.md`
- `src/lib.rs` (the router, `bearer_token`, `token_matches`,
  `try_acquire_publish_slot`, `latest_metadata`, `LatestMetadataResponse`)
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- `expire_leases` and the lease task from task 001
- The wrapped and direct body rule
- The `remoteValue` body shape
- `docs/adr/0001-reserved-live-items.md`

## Constraints

- The route order of checks is: bearer token, event, token match, rate limit,
  snapshot. The status codes are in ADR 0002.
- The route ignores a request body. It does not use `Json` extraction.
- A `200` sets `renewed_at` to `now` and `touch`es the event. It returns
  `event_id`, `lease_expires_at` and `keepalive_interval_secs`.
- A `409` has the error code `lease_expired`. It does not change
  `renewed_at`.
- The keepalive uses the publish rate limit of the event.
- `LatestMetadataResponse` adds `renewed_at` and `lease_expires_at`. Use the
  same time format as `updated_at`.
- `README.md` documents the route, each status code, the new response fields,
  `LEASE_SECS`, and the expiry behavior. Each example must be correct.
- `docs/interoperability.md` records that an event now also goes off air when
  its lease expires (AGENTS.md §6).

## Implementation Steps

1. Add `RelayState::keepalive(event_id, token) -> Result<KeepaliveResponse,
   ApiError>` with the order of checks in the constraints.
2. Add the handler and the route.
3. Add the two fields to `LatestMetadataResponse`.
4. Add tests in `tests/api.rs`, one for each status code:
   - `200` with the three fields, and a later `remoteValue` still gives the
     snapshot after the first lease duration would have ended,
   - `401` with no `Authorization` header,
   - `403` with a wrong token,
   - `404` for an unknown event,
   - `409` for an event with no snapshot, and `remoteValue` stays `{}`,
   - `409` after an expiry, and the old snapshot does not come back,
   - `429` after the publish rate limit is used.
5. Add a test that `GET .../metadata` has `renewed_at` and `lease_expires_at`.
6. Change `README.md` and `docs/interoperability.md`.

## Acceptance Criteria

- Each test in steps 4 and 5 exists and passes.
- Each status code of the route is in `README.md` and has a test.
- `docs/interoperability.md` names the lease as a third way for an event to go
  off air.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

Stop and report if one of these occurs:

- The route needs a body to satisfy a proxy or a client.
- The publish rate limit makes a keepalive at the interval fail.
- A README example cannot match the real response without a change to ADR
  0002.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0002-live-lease.md
- docs/tasks/live-lease-task-002-keepalive-route-and-docs.md
- src/lib.rs
- tests/api.rs
- README.md
- docs/interoperability.md
- /home/citizen/.agents/skills/asd-ste100/SKILL.md (for all document prose)

Goal:
- Add POST /v1/liveitems/{event_id}/keepalive and the lease fields on GET .../metadata. Document the contract.

Constraints:
- Check order: bearer token, event, token match, rate limit, snapshot. Status codes as ADR 0002 states.
- Ignore any request body. No Json extraction.
- 200 sets renewed_at to now, touches the event, and returns event_id, lease_expires_at, keepalive_interval_secs.
- 409 lease_expired does not change renewed_at.
- Use the publish rate limit.
- LatestMetadataResponse adds renewed_at and lease_expires_at in the updated_at format.
- README.md documents the route, each status code, the new fields, LEASE_SECS and the expiry. docs/interoperability.md records the lease as a way to go off air.

Do not touch:
- expire_leases and the lease task
- The wrapped and direct body rule
- The remoteValue body shape
- docs/adr/0001-reserved-live-items.md

Acceptance criteria:
- tests/api.rs: 200 with the three fields; 401; 403; 404; 409 with no snapshot; 409 after expiry with no snapshot restore; 429.
- A test that GET .../metadata has renewed_at and lease_expires_at.
- Each route status code is in README.md.

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
