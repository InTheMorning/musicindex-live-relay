# Display Album Task 001: The Album Key

Status: Implemented - 2026-10-06.

Every criterion is mechanical.

## Goal

A display track can carry `album`. The relay validates it and passes it
through without a change.

## Files To Inspect

- `docs/adr/0006-display-album.md`
- `docs/adr/0005-display-song-line-and-value.md`
- `src/lib.rs`: `validate_display`, `is_text_of_length`,
  `MAX_SONG_LINE_CHARS`
- `tests/api.rs`: the tests of `songLine` and `value`
- `README.md` (the display track), `docs/interoperability.md`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The live value routes, Socket.IO and the listener timeline
- The image store
- The body limit of the display route
- `docs/adr/**`

## Constraints

- Add `album` to the known track keys of `validate_display`.
- `album` is a string of 1 to 1,024 characters. Count characters, not bytes.
  Add the constant `MAX_ALBUM_CHARS` with a doc comment that names ADR 0006,
  and check the key with `is_text_of_length`.
- A `null` album gives `400 invalid_display`. A track with no album has no
  key.
- The stored and the sent state are the body as received.

## Implementation Steps

1. Add the constant and the check.
2. Add the tests.
3. Add `album` to the display track in `README.md` and
   `docs/interoperability.md`. Name ADR 0006.

## Acceptance Criteria

Each item is a test in `tests/api.rs`:

- A track with `album` is accepted, and `GET /display` gives it without a
  change.
- A track with `album`, `songLine` and `value` is accepted.
- An `album` of 1,024 two-byte characters is accepted.
- Each of these gives `400 invalid_display`: an empty `album`, an `album` of
  1,025 characters, a `null` album, and an `album` that is a number.
- An SSE subscriber of `/display/events` receives `album`.
- The existing display tests pass with no change.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- An existing test asserts that `album` is refused.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0006-display-album.md
- docs/adr/0005-display-song-line-and-value.md
- docs/tasks/display-album-task-001-album-key.md
- src/lib.rs
- tests/api.rs
- README.md, docs/interoperability.md

Goal:
- Accept the optional track key album in a display state, validate it, and pass it through.

Constraints:
- Follow §Constraints of the packet exactly.

Do not touch:
- The live value routes, Socket.IO, the listener timeline, the image store, the body limit, docs/adr/**

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

Reviewed 2026-10-06. `Cargo.lock` did not change. The full gate passes.

The escalation trigger of this packet occurred. The ADR 0003 test
`a_wrong_shape_or_a_wrong_url_gives_400` used `album` as its example of an
unknown track key. The implementation deleted that case. ADR 0006 makes
`album` valid, so the case asserted a replaced rule. But the deletion also
removed the only unknown-key case of the ADR 0003 test. The review put the
case back with the key `genre`, which stays unknown.

The five new tests cover each criterion:

- `a_track_with_album_is_accepted`
- `a_track_with_album_song_line_and_value_is_accepted`
- `an_album_of_1024_two_byte_characters_is_accepted`
- `invalid_album_gives_400`
- `a_subscriber_receives_album`
