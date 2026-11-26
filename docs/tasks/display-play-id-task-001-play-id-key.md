# Display Play Identifier Task 001: The `playId` Key

Status: Implemented - 2026-11-26.

Every criterion is mechanical. The deployment order is a gate of the review
checklist, not of this packet.

## Goal

A track of a display state can have the optional key `playId`: a string of
1 to 128 characters, or `null`. The relay passes it through without a change
(ADR 0012).

## Base components

| Component | Status | Location | Reason |
|---|---|---|---|
| The display state validation | extend | `src/lib.rs` | `validate_display` holds the list of track keys and the check of each key |
| The text length check | reuse | `src/lib.rs` | `is_text_of_length` checks a string of 1 to N characters, as for `songLine` and `album` |
| The display tests | extend | `tests/api.rs` | The tests of `album` (ADR 0006) are the pattern for a new optional key |

## Files To Inspect

- `docs/archive/adr/0012-display-play-id.md`
- `docs/archive/adr/0006-display-album.md`
- `docs/tasks/display-album-task-001-album-key.md`
- `src/lib.rs`: `validate_display`, `is_text_of_length`, `MAX_ALBUM_CHARS`,
  `MAX_VALUE_GUID_CHARS`
- `tests/api.rs`: each test that has `album` in its name or its body
- `README.md`: the display state of "Publish The Display State", and each
  place that names `album`
- `docs/interoperability.md`: each place that names `album`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The checks of `artist`, `title`, `artwork`, `songLine`, `value` and
  `album`.
- Each route, each status code and the body limit of 8 KiB.
- The live value routes and Socket.IO.
- `Cargo.toml`, `docs/adr/**`, `AGENTS.md`, `docs/README.md`.

## Constraints

- Add the constant `MAX_PLAY_ID_CHARS` with the value 128, with a doc comment
  that names ADR 0012.
- In `validate_display`, add `playId` to the known keys. A `playId` that is
  not `null` must pass `is_text_of_length` with `MAX_PLAY_ID_CHARS`.
- `null` is valid for `playId` only. Do not make `null` valid for `album` or
  `songLine`.
- The relay stores and sends the validated state as received. Do not add,
  remove or change `playId`. Do not read it in any other function.
- Update the comment in `validate_display` that lists the optional keys and
  their records, and the doc comment of `validate_display`.

## Implementation Steps

1. Add the constant and the check.
2. Add the tests of §Acceptance Criteria next to the `album` tests.
3. In `README.md`, add `playId` to the description of the display track: its
   shape, its limit, the `null` form and the pass-through. State that an
   empty string gives `400 invalid_display`.
4. In `docs/interoperability.md`, add `playId` next to `album`: publisher
   ADR 0038 sends it, and the relay must be deployed before a publisher sends
   it.

## Acceptance Criteria

Mechanical. Each item is a test in `tests/api.rs`:

- A track with a `playId` string is accepted, and `GET /display` gives it
  without a change.
- A track with `playId` `null` is accepted, and `GET /display` gives the key
  with `null`.
- A track with `playId`, `album`, `songLine` and `value` is accepted.
- A `playId` of 128 characters is accepted.
- Each of these gives `400 invalid_display`: an empty `playId`, a `playId` of
  129 characters, and a `playId` that is a number.
- An SSE subscriber of `/display/events` receives `playId`.
- Each existing test passes with no change to its assertions.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- An existing test must change its assertion.
- The change needs a new dependency or a change to a route.
- The pass-through needs a change outside `validate_display`.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/archive/adr/0012-display-play-id.md
- docs/tasks/display-play-id-task-001-play-id-key.md
- the files of §Files To Inspect of the packet

Goal:
- A display track accepts the optional key playId, a string of 1 to 128 characters or null, and the relay passes it through without a change.

Constraints:
- Follow §Constraints of the packet exactly.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.
- Do not commit. Leave the change in the working tree.

Do not touch:
- the checks of the other track keys, the routes, the live value routes, Cargo.toml, docs/adr/**, AGENTS.md, docs/README.md

Acceptance criteria:
- Each mechanical item in §Acceptance Criteria of the packet is a passing test.

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

Reviewed 2026-11-26. The full gate passes.

- A ChatGPT run made the change from this packet. The main session read each
  line of the diff and ran the gate.
- `validate_display` accepts `playId` as `null` or as a string that passes
  `is_text_of_length` with `MAX_PLAY_ID_CHARS`. No other function reads the
  key.
- No existing test changed. Six tests are new.
- The review found no defect.
