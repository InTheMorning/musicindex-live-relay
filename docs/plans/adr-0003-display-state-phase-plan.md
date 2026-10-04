# Display State And Artwork Phase Plan

Date: 2026-10-04. This plan states no rule. ADR 0003 owns the rules here.

## Goal

A reserved event gets an optional display state and an image store. When the
plan is complete, a broadcaster can upload an image, publish a display state,
and a private client can read both. Podcast apps see no change.

## Non-Goals

- A display path for an ephemeral event.
- A read token for the display routes.
- Any change to `remoteValue`, `/events`, Socket.IO or the metadata route.

## Assumptions

- ADR 0001 is implemented. An event knows if it is reserved.
- No client uses the relay in production.

## Affected Modules

| Module | Change | Task |
|---|---|---|
| `src/lib.rs` | The display state, its routes and its SSE stream | 001 |
| `src/lib.rs` | The image store, its routes and its limits | 002 |
| `tests/api.rs` | A test for each route and each status code | 001, 002 |
| `README.md`, `docs/interoperability.md` | The routes | 002 |

## Sequence

1. ADR 0001 tasks 001 to 005.
2. Task 001: the display state.
3. Task 002: the image store. It needs task 001, because a display publish
   checks that its image exists.

## Risk Areas

- **Memory.** Only a reserved event may store data, each write route has a
  body limit, and an event keeps two images at most.
- **Content type.** The relay checks the first bytes, and it serves each image
  with `nosniff`.
- **Lease.** A display publish must not renew the lease. A lease expiry must
  clear the display state and the images.
- **Live value isolation.** No display event may reach `/events` or
  Socket.IO.

## Test Strategy

- Integration tests in `tests/api.rs`, the same as the other routes.
- Lease tests use the injected clock. No test sleeps for a lease.
- Image tests use small byte arrays that start with the JPEG or PNG magic
  bytes.

## Rollback

Each task is one commit. A revert removes the routes. No data reaches disk, so
a revert needs no data step.

## Tasks

- `docs/tasks/display-state-task-001-state-and-routes.md`
- `docs/tasks/display-state-task-002-artwork-store.md`
