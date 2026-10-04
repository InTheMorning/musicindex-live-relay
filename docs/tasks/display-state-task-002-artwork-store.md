# Display State Task 002: The Artwork Store

Status: Ready - 2026-10-04. Do after task 001.

Every criterion is mechanical.

## Goal

A reserved event stores a maximum of two images. A broadcaster uploads an
image by its hash. A display publish refuses an image that the relay does not
hold. A client reads an image by its hash.

## Files To Inspect

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/tasks/display-state-task-001-state-and-routes.md`
- `src/lib.rs` (the display state from task 001, `expire_leases`, the router)
- `tests/api.rs`
- `README.md` and `docs/interoperability.md`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The live value routes and Socket.IO
- `Cargo.toml`. `sha2` is already a dependency.
- `docs/adr/**`

## Constraints

- `PUT /v1/liveitems/{event_id}/artwork/{sha256}`:
  - It needs the broadcaster token and uses the publish rate limit.
  - Body limit `ARTWORK_MAX_BYTES`, default 524,288, from the environment,
    with `RequestBodyLimitLayer`. Over the limit gives `413`.
  - `{sha256}` must be 64 lowercase hex characters, and the SHA-256 of the
    body must be equal to it. Else `400 sha256_mismatch`.
  - The body must start with the JPEG bytes `FF D8 FF` or the PNG bytes
    `89 50 4E 47 0D 0A 1A 0A`. Else `400 unsupported_image`. The type comes
    from these bytes only.
  - An image that the event holds already gives `200` with no change.
- `GET /v1/liveitems/{event_id}/artwork/{sha256}`:
  - It gives the bytes with `Content-Type` from the stored type,
    `Cache-Control: public, max-age=31536000, immutable` and
    `X-Content-Type-Options: nosniff`.
  - An image that the event does not hold gives `404`.
- The display publish of task 001 gives `409 artwork_missing` for an
  `artwork.sha256` that the event does not hold. Its `mime` must agree with
  the stored type, else `400 invalid_display`.
- Retention: an event keeps the image of its present display state and the
  image of the state before it. After a display publish, it removes every
  other image. An uploaded image that no display state names yet also stays
  until the next display publish.
- A lease expiry removes every image of the event.
- Every artwork route gives `404 event_not_found` and
  `409 event_not_reserved` as task 001 does.
- `README.md` and `docs/interoperability.md` describe the display and artwork
  routes, their codes and their limits, in this commit.

## Implementation Steps

1. Add the image store to the event state.
2. Add the two routes.
3. Add the `artwork_missing` check and the retention rule.
4. Add the lease expiry rule.
5. Add the tests and the documentation.

## Acceptance Criteria

Each item is a test in `tests/api.rs`:

- An upload and a read give the same bytes and the three headers.
- A wrong hash gives `400 sha256_mismatch`.
- Bytes that are not JPEG or PNG give `400 unsupported_image`.
- A body over the limit gives `413`.
- A second upload of one image gives `200` with no change.
- A display publish with an image that the event does not hold gives
  `409 artwork_missing`.
- After three display states with three images, the first image gives `404`
  and the last two are present.
- A lease expiry removes the images.
- An ephemeral event gives `409 event_not_reserved` on both routes.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The retention rule needs an image count other than two.
- A rule here conflicts with ADR 0003.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan. Implement only this
task. Do not redesign the architecture.

Read docs/adr/0003-display-state-and-artwork.md, this packet, task 001,
src/lib.rs, tests/api.rs, README.md and docs/interoperability.md.

Implement the artwork store and its two routes for reserved events, exactly as
the packet §Constraints says. Write each test in §Acceptance Criteria. Update
README.md and docs/interoperability.md. Do not add a dependency. Run the test
commands.

At the end, report: 1. files changed 2. tests run 3. behavior changed
4. deviations from task 5. unresolved concerns.
