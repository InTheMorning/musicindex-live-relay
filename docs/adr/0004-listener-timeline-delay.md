# ADR 0004: A Delayed Listener Timeline For Socket.IO

## Status

Proposed 2026-10-06.

Class: situational. Supersede this record when podcast apps get a sync signal
that is better than one fixed delay, or when the Socket.IO path stops.

## Context

`docs/interoperability.md` §Per-transport delay for live payloads records an
open question. Today `musicindex-live-publisher` holds each payload for a
stream delay before it sends it here. Each transport of this relay thus gets
the delayed payload.

Two kinds of consumer need opposite timing:

- **A consumer that applies a block when it arrives.** Podcast apps read the
  Socket.IO `remoteValue` event of `podcast:liveValue`. They apply and pay
  each block at once. For them, the delay is the only approximation of the
  time that the listener hears the track. This path works with current apps,
  and it must continue to work as before.
- **A consumer that waits for an in-band key.** The private app holds display
  states. It applies one when the ICY title names it (`citizenradio` ADR
  0011). The tagger of publisher ADR 0009 does the same on the VPS for HLS.
  These consumers need each block before its key arrives, and a delay makes
  them late.

  The ICY title reaches the VPS about 1 to 3 seconds after the track change.
  With the delay, the tagger is late by almost the full delay.

The SSE routes of this relay have no known consumer in a podcast app. The
operator decided on 2026-10-06 that they become instant, because they are new
and no deployed client depends on their timing.

The broadcaster knows the stream. The broadcaster thus sets the delay. `v4vmm` lets the operator change it, through the publisher.

## Decision

### Two Timelines

Each event has two timelines of the same updates:

- **The instant timeline.** Each publish, each lease expiry and each display
  update is on it at the time that the relay accepts it.
- **The listener timeline.** It has the same updates of the live value in the
  same order. Each one is on it a delay after it was on the instant timeline.

The routes use the timelines as follows:

| Route | Timeline |
|---|---|
| Socket.IO `remoteValue`, also the block sent on connect | Listener |
| `GET /v1/liveitems/{event_id}/remoteValue` | Listener |
| `GET /v1/liveitems/{event_id}/events` (SSE) | Instant |
| `GET /v1/liveitems/{event_id}/metadata` | Instant |
| `GET /v1/liveitems/{event_id}/display` and `/display/events` | Instant |

`GET /remoteValue` follows Socket.IO, because a podcast app that polls it
expects the same timing. `GET /metadata` is the view of `v4vmm` and gives the
present truth. The display routes have no Socket.IO form, so they are instant.

### The Delay

- The broadcaster sends the delay with each publish, in the request header
  `Listener-Delay-Secs`. The value is a whole number of seconds from 0 to
  `MAX_LISTENER_DELAY_SECS`.
- The body does not include the header. The two body forms and their separation
  by exact key match do not change.
- A publish with no header has the delay 0. A broadcaster that does not send
  the header thus gets the behavior of today.
- A value that is not a whole number, or that is more than the limit, gives
  `400` with the code `invalid_listener_delay`.
- The relay does not store the delay. Each publish brings it again, so a relay
  restart needs no restore of the delay.

### The Order On The Listener Timeline

- An update goes onto the listener timeline at its instant time plus the delay
  of its publish.
- That time is never earlier than the time of the update before it on the
  listener timeline. When a shorter delay follows a longer one, the update
  waits. The order of the instant timeline is thus kept.
- A lease expiry has the delay of the publish before it. The `{}` of ADR 0002
  §Expiry thus reaches Socket.IO after the last block, never before it.

### The Pending Updates

- An event holds at most `MAX_PENDING_LISTENER_UPDATES` updates that are not
  yet on the listener timeline.
- When a publish finds the list full, the new update replaces the newest
  pending update. The listener timeline then skips one block, but its last
  value is correct.
- The pending updates are in memory only. A restart removes them, the same as
  the snapshot.

### The Lease And The `404`

- ADR 0002 does not change on the instant timeline. `GET /metadata` gives
  `404 metadata_not_found` at expiry, as before.
- On the listener timeline, the `{}` of the expiry arrives after the delay.
  Until then, `GET /remoteValue` and a new Socket.IO client get the last block
  on the listener timeline.
- When the relay deletes an event, it removes the pending updates of that
  event, and no transport sends them.

### Configuration

| Variable | Default | Function |
|---|---|---|
| `MAX_LISTENER_DELAY_SECS` | `300` | The highest delay that a publish can set |
| `MAX_PENDING_LISTENER_UPDATES` | `64` | The pending updates for each event |

The default of 300 seconds is the limit that the publisher uses today.

## Invariants

These rules apply while this decision is current.

- The Socket.IO payload and its shape do not change. Only its time changes.
- The listener timeline has the updates of the instant timeline in the same
  order.
- The SSE routes and `GET /metadata` never wait for the listener delay.
- A publish with no `Listener-Delay-Secs` header behaves as before this ADR.
- No transport sends an update of a deleted event.

## Before Acceptance

1. **The publisher side.** `musicindex-live-publisher` ADR 0011 is accepted.
   It retires the delay in the publisher. Without it, the delay applies two
   times.
2. **The interoperability record.** `docs/interoperability.md` names this ADR
   as the answer to §Per-transport delay for live payloads, and lists the new
   timing of each route.
3. **The `v4vmm` view.** `v4vmm` reads `GET /metadata` to show what listeners
   receive. After this ADR it shows the present truth. Make sure that `v4vmm`
   accepts that, or that it reads `GET /remoteValue` instead.

## Verification After Implementation

Mechanical. Each test uses an injected clock, not a wall-clock sleep:

- A publish with a delay reaches SSE at once and Socket.IO after the delay.
- `GET /remoteValue` gives the old block until the delay ends, then the new
  one. `GET /metadata` gives the new block at once.
- A short delay after a long delay keeps the order on the listener timeline.
- A lease expiry gives `{}` on Socket.IO after the last block, and `404` on
  `GET /metadata` at once.
- A full pending list replaces the newest pending update.
- A publish with no header has the delay 0.
- `400 invalid_listener_delay` for a value that is not a whole number and for
  a value over the limit.
- A deleted event sends no pending update.

Visual. A person must examine this item with a podcast app. Report it as open
until a person completes the check.

- A podcast app on the Socket.IO path changes the block at the same time as
  before the change, with the same delay value.

## Alternatives Considered

### The Delay Stays In The Publisher

Rejected. Every transport then gets the delay, and a consumer that waits for
an in-band key is late.

### Two Events, One Delayed And One Instant

Rejected. Each track then needs two publishes, and a drop file names one
target only. One event with two timelines needs no fan-out.

### A Separate Route That Sets The Delay

Rejected. The relay must then store the delay. A reserved event then needs a
schema change to keep the delay across a restart. Without that change, a
restart removes the delay. A header
on each publish needs no storage.

### SSE Follows The Delay Too

Rejected. No deployed client depends on the SSE timing, and the consumers
that wait for an in-band key need it instant.

## Consequences

Positive:

- Podcast apps keep the same timing on Socket.IO.
- The ICY-sync fallback and the tagger get each block before its key.
- The open question in `docs/interoperability.md` gets an answer.

Negative and risks:

- The relay holds pending updates and runs a timer for each event. That is
  new state and a new bound.
- The relay now knows about the broadcast delay. Publisher audit fix 006 kept
  the delay out of the relay intentionally, to keep this relay a plain
  fan-out.
- A client that pays from SSE pays before the listener hears the track. Only a client that waits for an
  in-band key may pay from SSE. `README.md` must state this for client
  authors.

## References

- `docs/adr/0001-reserved-live-items.md`
- `docs/adr/0002-live-lease.md`
- `docs/adr/0003-display-state-and-artwork.md`
- `docs/interoperability.md`
- `musicindex-live-publisher`: `docs/adr/0005-producer-liveness-and-dead-block.md`
- `musicindex-live-publisher`: `docs/adr/0009-hls-track-metadata.md`
- `musicindex-live-publisher`: `docs/adr/0011-relay-applies-stream-delay.md`
- `musicindex-live-publisher`: `docs/tasks/audit-fix-task-006-stream-delay-compensation.md`
- `citizenradio`: `docs/adr/0011-icy-sync-of-the-relay-display.md`
