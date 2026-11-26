# Display Publication Date Task 001: The `pubDate` Key

Status: Implemented - 2026-11-26.

Every criterion is mechanical. The deployment order is a gate of the review
checklist, not of this packet.

## Goal

A track of a display state can have the optional key `pubDate`. It is a
string of exactly 10 characters in the form `YYYY-MM-DD`, and it is a
calendar day. The relay passes it through without a change (ADR 0013).

## Base components

| Component | Status | Location | Reason |
|---|---|---|---|
| The display state validation | extend | `src/lib.rs` | `validate_display` holds the list of track keys and the check of each key |
| The calendar check | reuse | `Cargo.toml` | The crate `time` is a dependency. `time::Date::from_calendar_date` refuses a day that does not exist |
| The display tests | extend | `tests/api.rs` | The tests of `playId` (ADR 0012) are the pattern for a new optional key |

## Files To Inspect

- `docs/adr/0013-display-track-optional-keys.md`
- `docs/tasks/display-play-id-task-001-play-id-key.md`
- `src/lib.rs`: `validate_display`, `is_text_of_length`, `MAX_PLAY_ID_CHARS`,
  and each use of the crate `time`
- `tests/api.rs`: each test that has `play_id` or `playId` in its name or
  its body
- `README.md`: the display state of "Publish The Display State", and each
  place that names `playId`
- `docs/interoperability.md`: each place that names `playId`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The checks of `artist`, `title`, `artwork`, `songLine`, `value`, `album`
  and `playId`.
- Each route, each status code and the body limit of 8 KiB.
- The live value routes and Socket.IO.
- `Cargo.toml`, `docs/adr/**`, `docs/archive/**`, `AGENTS.md`,
  `docs/README.md`. The main session moves the superseded records.

## Constraints

- Add one function, for example `fn is_calendar_day(value: &Value) -> bool`,
  with a doc comment that names ADR 0013 §The Form Of `pubDate`. It gives
  `true` only when all of these conditions are true:
  - The value is a string of exactly 10 bytes.
  - Bytes 4 and 7 are `-`. Each other byte is an ASCII digit.
  - `time::Date::from_calendar_date` accepts the year, the month and the
    day.
- Do not add the `parsing` feature of `time`, and do not add a dependency.
- In `validate_display`, add `pubDate` to the known keys. A `pubDate` must
  pass the new function. `null` is not valid for `pubDate`.
- The relay stores and sends the validated state as received. Do not add,
  remove or change `pubDate`. Do not read it in any other function.
- Do not compare the day with the clock.
- Update the comment in `validate_display` that lists the optional keys.
  Update the doc comment of `validate_display` also. Name ADR 0013 as the owner of each
  optional key. ADR 0013 supersedes ADR 0005, ADR 0006 and ADR 0012. Remove
  those three names from the comments in `src/lib.rs`.

## Implementation Steps

1. Add the function and the check, with unit tests of the function.
2. Add the tests of §Acceptance Criteria adjacent to the `playId` tests.
3. In `README.md`, add `pubDate` to the example and to the description of
   the display track: its form, the calendar check, no `null`, and the
   pass-through. Name ADR 0013 as the owner of each optional key, in place
   of ADR 0005, ADR 0006 and ADR 0012.
4. In `docs/interoperability.md`, add `pubDate` adjacent to `playId`: publisher
   ADR 0042 sends it, and the relay must be deployed before a publisher
   sends it. Name ADR 0013 in place of ADR 0005, ADR 0006 and ADR 0012.

## Acceptance Criteria

Mechanical. Each item is a test in `tests/api.rs`, unless the item names a
unit test:

- A track with `pubDate` `2026-11-12` is accepted, and `GET /display` gives
  it without a change.
- A track with `pubDate`, `playId`, `album`, `songLine` and `value` is
  accepted.
- A track with `pubDate` `2024-02-29` is accepted.
- Each of these gives `400 invalid_display`:
  - an empty `pubDate`, a `pubDate` that is `null`, and a `pubDate` that is
    a number,
  - `2026-02-30`, `2026-13-01` and `2026-00-10`,
  - `2026-1-5`, `20261112` and `2026/11/12`,
  - `2026-11-12T00:00:00Z`.
- An SSE subscriber of `/display/events` receives `pubDate`.
- Unit test: the function gives `false` for `2025-02-29` and for a string of
  10 characters with a digit that is not an ASCII digit, for example
  `２026-11-1`.
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
- The change needs a new dependency, a new feature of a crate or a change to
  a route.
- The pass-through needs a change outside `validate_display`.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0013-display-track-optional-keys.md
- docs/tasks/display-pub-date-task-001-pub-date-key.md
- the files of §Files To Inspect of the packet

Goal:
- A display track accepts the optional key pubDate, a calendar day in the form YYYY-MM-DD, and the relay passes it through without a change.

Constraints:
- Follow §Constraints of the packet exactly.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.
- Do not commit. Leave the change in the working tree.

Do not touch:
- the checks of the other track keys, the routes, the live value routes, Cargo.toml, docs/adr/**, docs/archive/**, AGENTS.md, docs/README.md

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
- `validate_display` accepts `pubDate` only when `is_calendar_day` gives
  `true`. No other function reads the key.
- No existing test changed its assertions.
- The review found no defect.
