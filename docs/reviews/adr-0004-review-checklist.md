# ADR 0004 Review Checklist

Status: open. Use this checklist after task 003. ADR 0004 becomes
`Implemented` only when each item passes.

## Invariants

- [ ] The Socket.IO payload and its shape do not change. Only its time
  changes.
- [ ] The listener timeline has the updates of the instant timeline in the
  same order. A test covers a short delay after a long one.
- [ ] SSE, `GET /metadata` and the display routes never wait for the delay.
- [ ] A publish with no `Listener-Delay-Secs` header behaves as before. A test
  shows that `GET /remoteValue` changes with no sweep.
- [ ] No transport sends an update of a deleted event.

## Code

- [ ] No lock is held across a Socket.IO emit or an SSE send.
- [ ] The pending list has its bound, and a test fills it.
- [ ] Each new limit has a default constant and an `AppConfig` field.
- [ ] No test uses a wall-clock sleep.
- [ ] The two body forms and their separation do not change.

## Documents

- [ ] `README.md` names the header, `400 invalid_listener_delay`, the two
  variables and the timing of each route.
- [ ] `README.md` says that only a client that waits for an in-band key may
  pay from SSE.
- [ ] `docs/interoperability.md` and `AGENTS.md` describe the present timing.
- [ ] Each status code in `README.md` has a test.

## Cross-Repository

- [ ] `musicindex-live-publisher` ADR 0011 is accepted. Without it, the delay
  applies two times.
- [ ] The relay is deployed before a publisher that sends the header.

## Visual

A person must complete this item. Report it as open until then.

- [ ] A podcast app on Socket.IO changes the block at the same time as before.
