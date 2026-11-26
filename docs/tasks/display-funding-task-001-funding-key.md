# Display Funding Task 001: The `funding` Key

Status: Implemented - 2026-11-26.

Every criterion is mechanical. The deployment order is a gate of the review
checklist, not of this packet.

This packet can run in one change with
`docs/tasks/display-pub-date-task-001-pub-date-key.md`. One agent then does
the two packets, the `pubDate` packet first.

## Goal

A track of a display state can have the optional key `funding`. It is an
array of 1 to 3 entries, each with a `url` and an optional `label`. The
relay passes it through without a change (ADR 0013 §The Form Of `funding`).

## Base components

| Component | Status | Location | Reason |
|---|---|---|---|
| The display state validation | extend | `src/lib.rs` | `validate_display` holds the list of track keys and the check of each key |
| The check of an `http` or `https` URL | extend | `src/lib.rs` | `is_valid_artwork_url` checks the scheme and the length of `artwork.url`. The `funding` URL needs the same check with a different limit and a host check |
| The text length check | reuse | `src/lib.rs` | `is_text_of_length` checks a string of 1 to N characters |
| The display tests | extend | `tests/api.rs` | The tests of `playId` (ADR 0012) are the pattern for a new optional key |

## Files To Inspect

- `docs/adr/0013-display-track-optional-keys.md`, §The Form Of `funding`
  and §Verification After Implementation
- `src/lib.rs`: `validate_display`, `is_valid_artwork_url`,
  `is_text_of_length`, `MAX_ARTWORK_URL_CHARS`
- `tests/api.rs`: each test that has `play_id` or `playId` in its name or
  its body, and each test of `artwork.url`
- `README.md`: the display state of "Publish The Display State"
- `docs/interoperability.md`: each place that names `playId`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The result of each existing check. `artwork.url` keeps its rule: the
  scheme, the limit of 2,048 characters and a part after the scheme. Do not
  add the host check or the white space check to `artwork.url`.
- Each route, each status code and the body limit of 8 KiB.
- The live value routes and Socket.IO.
- `Cargo.toml`, `docs/adr/**`, `docs/archive/**`, `AGENTS.md`,
  `docs/README.md`.

## Constraints

- Add the constants `MAX_FUNDING_ENTRIES` (3), `MAX_FUNDING_URL_CHARS` (512)
  and `MAX_FUNDING_LABEL_CHARS` (128). Each doc comment names ADR 0013.
- Do not make a second copy of the scheme check. Divide
  `is_valid_artwork_url` into one shared function for the scheme and the
  length, and use it for `artwork.url` and for the `funding` URL. An example
  is `fn http_url_rest(url: &str, max_chars: usize) -> Option<&str>`, which
  gives the text after `://`.
- The `funding` URL also needs these checks:
  - One character or more comes before the first `/`, `?` or `#` of the
    text after `://`.
  - No character of the URL gives `true` for `char::is_whitespace` or
    `char::is_control`.
- A `label` must pass `is_text_of_length` with `MAX_FUNDING_LABEL_CHARS`,
  and must be equal to its `str::trim` result.
- An entry is an object with `url`, an optional `label` and no other key.
- `funding` is an array of 1 to `MAX_FUNDING_ENTRIES` entries. `null` and an
  empty array are not valid.
- Two entries with the same `url` string are not valid. Compare the strings
  with `==`. Do not change the case and do not parse the URL.
- Put the check of `funding` in one function, for example
  `fn is_valid_funding(value: &Value) -> bool`, and call it from
  `validate_display`.
- The relay stores and sends the validated state as received. Do not add,
  remove or change an entry. Do not read `funding` in any other function.
- Do not add a dependency. Do not parse the URL with a crate.
- Update the comment in `validate_display` that lists the optional keys.
  Update the doc comment of `validate_display` also.

## Implementation Steps

1. Divide `is_valid_artwork_url`. Run the tests of `artwork.url`. They must
   pass with no change.
2. Add the constants, the function and the check, with unit tests of the
   function.
3. Add the tests of §Acceptance Criteria adjacent to the `playId` tests.
4. In `README.md`, add `funding` to the example and to the description of
   the display track. Give each rule of ADR 0013 §The Form Of `funding`.
5. In `docs/interoperability.md`, add `funding` adjacent to `playId`.
   Publisher ADR 0043 sends it. The relay must be deployed before a
   publisher sends it.

## Acceptance Criteria

Mechanical. Each item is a test in `tests/api.rs`, unless the item names a
unit test:

- A track with two entries, one with `label` and one with no `label`, is
  accepted, and `GET /display` gives the array without a change.
- A track with three entries is accepted.
- A track with `funding`, `playId`, `album`, `songLine` and `value` is
  accepted. If the `pubDate` packet is done, the track also has `pubDate`.
- A `url` of 512 characters and a `label` of 128 characters are accepted.
- A `url` with the scheme `HTTPS` is accepted.
- Each of these gives `400 invalid_display`:
  - `funding` that is `null`, an empty array, an object or a string,
  - four entries,
  - an entry that is not an object, an entry with no `url`, and an entry
    with a third key,
  - a `url` with the scheme `ftp`, `https:///support`, `https://?a`, a `url`
    of 513 characters, an empty `url`, a `url` that is a number and a `url`
    with a space,
  - an empty `label`, a `label` of 129 characters, a `label` with a space at
    its end, a `label` with a space at its start, and a `label` that is
    `null`,
  - two entries with the same `url`.
- An SSE subscriber of `/display/events` receives `funding`.
- Unit test: the function gives `false` for a `url` with a newline and for a
  `url` with a tab.
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
- The shared URL function changes the result of an `artwork.url` check.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0013-display-track-optional-keys.md
- docs/tasks/display-funding-task-001-funding-key.md
- the files of §Files To Inspect of the packet

Goal:
- A display track accepts the optional key funding, an array of 1 to 3 entries with a url and an optional label, and the relay passes it through without a change.

Constraints:
- Follow §Constraints of the packet exactly.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.
- Do not commit. Leave the change in the working tree.

Do not touch:
- the result of each existing check, the routes, the live value routes, Cargo.toml, docs/adr/**, docs/archive/**, AGENTS.md, docs/README.md

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
- `http_url_rest` holds the scheme check and the length check for
  `artwork.url` and for the `funding` URL. The rule of `artwork.url` did not
  change.
- `validate_display` accepts `funding` only when `is_valid_funding` gives
  `true`. No other function reads the key.
- The main session changed the test URL of 514 characters to 513 characters,
  and added two rules to the `README.md` text.
