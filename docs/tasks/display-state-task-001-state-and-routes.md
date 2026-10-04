# Display State Task 001: The Display State And Its Routes

Status: Implemented - 2026-10-04.

Every criterion is mechanical.

## Goal

A reserved event has a display state. A broadcaster publishes it, and a client
reads it and subscribes to it. An ephemeral event refuses the routes.

## Files To Inspect

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/adr/0002-live-lease.md`
- `src/lib.rs` (the metadata publish, `RelayState`, `expire_leases`, the
  replay buffer, the SSE handler for `/events`, the router)
- `tests/api.rs`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`

## Do Not Touch

- The metadata route, `remoteValue`, `/events` and Socket.IO behavior
- `docs/adr/**`

## Constraints

- `POST /v1/liveitems/{event_id}/display`:
  - It needs the broadcaster token, with the same checks and codes as a
    publish.
  - Body limit 8 KiB with `RequestBodyLimitLayer`, as for the metadata route.
  - It uses the publish rate limit.
  - It validates the display state of ADR 0003 §The Display State. A wrong
    shape gives `400 invalid_display`. An `artwork.url` with a scheme other
    than `http` or `https`, or longer than 2,048 characters, gives
    `400 invalid_display`.
  - In this task, an `artwork.sha256` that is 64 lowercase hex characters and
    a `mime` of `image/jpeg` or `image/png` pass validation. Task 002 adds the
    `409 artwork_missing` check.
  - It does not renew the lease and does not touch `renewed_at`.
- `GET /v1/liveitems/{event_id}/display` gives the present state, or
  `{"track": null}`.
- `GET /v1/liveitems/{event_id}/display/events` is an SSE stream of `display`
  events. It has its own sequence number and its own replay buffer, the same
  pattern as `/events`, with `Last-Event-ID`.
- Every display route gives `404 event_not_found` for an unknown event and
  `409 event_not_reserved` for an ephemeral event.
- When `expire_leases` ends the lease of an event, it sets the display state
  to `{"track": null}` and sends it on `/display/events`. Hold no lock across
  a send, as ADR 0002 already requires.
- The display state stays in memory. A restart gives `{"track": null}`.

## Implementation Steps

1. Add the display state to the event state, with its sequence number and its
   replay buffer.
2. Add the three routes.
3. Add the lease expiry rule.
4. Add the tests.

## Acceptance Criteria

Each item is a test in `tests/api.rs`:

- A publish and a read give the same state. `{"track": null}` is accepted.
- An SSE subscriber receives each display state. A reconnect with
  `Last-Event-ID` receives the missed states.
- An ephemeral event gives `409 event_not_reserved` on each route.
- A wrong token gives `403`, a missing token `401`, an unknown event `404`.
- A body over 8 KiB gives `413`.
- A wrong shape, a `data:` URL and a URL over 2,048 characters each give
  `400 invalid_display`.
- A display publish does not move `lease_expires_at`.
- A lease expiry sets the state to `{"track": null}` and sends it.
- No display event appears on `/events`.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The event state does not know if it is reserved.
- The SSE pattern of `/events` cannot carry a second stream without a large
  change.
- A rule here conflicts with ADR 0003.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan. Implement only this
task. Do not redesign the architecture.

Read docs/adr/0003-display-state-and-artwork.md, docs/adr/0002-live-lease.md,
this packet, src/lib.rs and tests/api.rs.

Implement the display state and its three routes for reserved events, exactly
as the packet §Constraints says. Write each test in §Acceptance Criteria. Do
not touch the live value routes or the ADRs. Run the test commands.

At the end, report: 1. files changed 2. tests run 3. behavior changed
4. deviations from task 5. unresolved concerns.

## Review Result

Reviewed 2026-10-04. The present tests did not change. `Cargo.lock` did not
change. The full gate passes.

The review added one change. The first version cleared the display state only
when a lease expiry also cleared a snapshot. An event with a display state and
no snapshot then kept its display state. ADR 0003 says that the display state
clears when the lease ends. `expire_leases` now also clears the display state
of an event with no snapshot. A test covers this case, and it fails when the
clear is removed. The count that `expire_leases` gives still counts only the
cleared snapshots.

Lock order: the display code takes only the `display` lock. It never holds
the `display` lock and the `inner` lock at the same time. It sends after it
releases the lock.

The review accepts these decisions of the task:

- The display publish gives `{event_id, accepted, seq}`.
- An expiry with a display state that is `null` already sends nothing.
- The root of the body holds only `track`. An unknown key gives
  `400 invalid_display`. Publisher display task 004 must not send the
  `schema` key of `display.json`.
- `README.md` and `docs/interoperability.md` changed in this task, not in
  task 002.

Known limits:

- The replay buffer holds at most 100 display states of 8 KiB for each
  reserved event. That is about 800 KiB. The memory limit in ADR 0003 counts
  only the images.
- Socket.IO has no runtime test. The display code has its own sender and
  never calls the Socket.IO emit.
