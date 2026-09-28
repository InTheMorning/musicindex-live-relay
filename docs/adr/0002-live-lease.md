# ADR 0002: A Live Event Is A Lease

## Status

Implemented - 2026-09-28.

Implemented 2026-09-28: live lease tasks 001 and 002 are merged. The review is
`docs/reviews/live-lease-review-checklist.md`.

Accepted 2026-09-27.

Proposed 2026-09-27. The operator accepted it on the same day.

This ADR comes from `docs/plans/live-lease-heartbeat-proposal.md`. It answers
the five open questions in that proposal.

## Context

A live event stays "on" until something turns it off. When the broadcaster
forgets, or its software stops, nothing turns it off:

- An ephemeral event expires after the idle TTL of 24 hours. A listener
  subscription calls `touch`, so an event with listeners never expires.
- A reserved event (ADR 0001) ignores the idle TTL.
- `remoteValue` gives the latest snapshot with no age limit. A listener that
  connects some hours after the broadcaster stopped gets the payment
  destinations of the last track.

AGENTS.md §4 calls a stale snapshot a payment defect. Today the relay applies
that rule only after a restart.

`musicindex-live-publisher` ADR 0005 is the broadcaster side. The publisher
always publishes an explicit block while its producer runs. When nobody may
be paid, it publishes a dead block that pays nobody. So the relay does not
need a stop message to clear a payee at once.

No client uses the relay in production. No transition period is necessary.

## Decision

### The Lease

- Each event has a `renewed_at` time in memory. A publish and a keepalive set
  it to the present time.
- The event is on air while it has a snapshot and `now - renewed_at` is less
  than the lease duration.
- `LEASE_SECS` sets the lease duration. The default is 90 seconds. The value
  must be 10 or more.
- The keepalive interval is `LEASE_SECS / 3`, rounded down. The relay tells
  the broadcaster this value. The broadcaster does not choose it.

### Expiry

When the lease of an event with a snapshot expires, the relay:

1. removes the snapshot,
2. increments `seq`,
3. adds an update with the body `{}` to the replay buffer,
4. sends `{}` one time to the SSE and Socket.IO subscribers.

After that:

- `remoteValue` gives `{}`.
- `GET /v1/liveitems/{event_id}/metadata` gives `404` with
  `metadata_not_found`.
- A client that reconnects with `Last-Event-ID` receives the `{}` update from
  the replay buffer.
- The event stays. A reserved event keeps its identifier and token.

A background task examines the leases each second. A test calls the same
expiry function with an injected clock.

### The Keepalive Route

`POST /v1/liveitems/{event_id}/keepalive`

- It needs the broadcaster token as a bearer token, the same as a publish.
- It has no body. The relay ignores a body.
- It uses the same rate limit as a publish.

| Status | Body `error` | Meaning |
|---|---|---|
| `200` | | The lease is renewed. |
| `401` | `missing_bearer_token`, `invalid_bearer_token` or `invalid_authorization_header` | No valid bearer token. The same codes as a publish. |
| `403` | `invalid_token` | The token does not match. |
| `404` | `event_not_found` | The event does not exist. |
| `409` | `lease_expired` | The event has no snapshot. The broadcaster must publish. |
| `429` | `publish_rate_limited` | Too many requests. |

A `200` body:

```json
{
  "event_id": "…",
  "lease_expires_at": "2026-09-27T18:00:00Z",
  "keepalive_interval_secs": 30
}
```

A keepalive never brings back a removed snapshot. That is the reason for
`409`. Only a publish makes an event go on air again.

The keepalive is a new route, not an empty publish. Today `{}` on the
metadata route is a valid snapshot. A second meaning for it would change a
public contract.

### Wire Changes

These changes add fields. They do not rename or remove a field.

- The publish response adds `lease_secs` and `keepalive_interval_secs`.
- `GET /v1/liveitems/{event_id}/metadata` adds `renewed_at` and
  `lease_expires_at` while the event is on air.
- The `remoteValue` body and the SSE and Socket.IO payload shape do not change.
  `{}` is already the body for an event with no snapshot.

`docs/interoperability.md` and `README.md` record these changes in the same
commit as the code (AGENTS.md §3 and §6).

## Invariants

- Only a publish or a keepalive with a valid token renews a lease. A listener
  never renews it.
- A keepalive never restores a removed snapshot.
- After a lease expires, no transport serves the old snapshot. This includes
  the replay buffer.
- The lease state is in memory only. A restart does not restore it, and the
  relay does not restore a snapshot after a restart.
- The keepalive route needs a credential and stores nothing durable.

## Non-Goals

- A stop message from the broadcaster. The publisher dead block clears a payee
  at once.
- A lease duration that the broadcaster chooses.
- A transition period for a broadcaster with no keepalive.
- An `on_air` field in `remoteValue`. That body is the Podcasting 2.0 payload.

## Alternatives Considered

### An Empty Publish As The Keepalive

Rejected. `{}` on the metadata route is a valid snapshot today. A second
meaning would break the rule that a direct payload passes through without
interpretation.

### Keep The Snapshot After Expiry And Add An `on_air` Flag

Rejected. A client that does not read the flag pays the stale destination.
Removal is safe for each client.

### A Broadcaster-Chosen Interval

Rejected for now. It adds validation and limits, and no client needs it.

### A Stop Message

Deferred. The dead block in `musicindex-live-publisher` ADR 0005 gives the
immediate clear. A stop message can come later if a broadcaster needs "off air
now".

## Consequences

Positive:

- A stopped broadcaster goes off air after one lease, for ephemeral and
  reserved events.
- A listener that connects late never gets a stale payee.
- The broadcaster needs only one new call.

Negative and risks:

- A broadcaster must send a keepalive each 30 seconds by default. A network
  outage longer than the lease takes the event off air. The next publish
  brings it back.
- The replay buffer holds one more update for each expiry.
- The reserved items work in ADR 0001 must keep `renewed_at` in memory and
  must not store it.

## References

- `docs/plans/live-lease-heartbeat-proposal.md`
- `docs/plans/adr-0002-live-lease-phase-plan.md`
- `docs/adr/0001-reserved-live-items.md`
- `docs/interoperability.md`
- `musicindex-live-publisher`: `docs/adr/0005-producer-liveness-and-dead-block.md`
