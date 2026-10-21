# State File Mode Task 001: The Private State File

Status: Implemented - 2026-10-21.

Every criterion is mechanical, except the error path. §Acceptance Criteria
says why.

## Goal

The relay makes the state file with the mode `0600`. An existing state
file can give a permission to the group or to other users. At each start the
relay then sets `0600` and logs one warning. A start that cannot set the mode fails
with an error that names the path (ADR 0009 §Decision).

## Files To Inspect

- `docs/adr/0009-state-file-mode.md`
- `src/store.rs`: `SqliteEventStore::open`, `StoreError` and its `Display`,
  the test helper `open_store` and the tests near it
- `src/lib.rs`: `RelayState::try_new` and each caller of
  `SqliteEventStore::open`, the startup log lines near it
- `docs/runbooks/reserved-live-items.md`: the backup and the deployment
  procedures
- `README.md`: the text on `STATE_FILE`

## Files Likely To Change

- `src/store.rs`
- `src/lib.rs`
- `docs/runbooks/reserved-live-items.md`
- `README.md`

## Do Not Touch

- The schema of the state file, `SCHEMA` and `SCHEMA_VERSION`.
- The content that the store writes, and each route.
- `Cargo.toml`. Use `std` only. Do not add `libc` or `nix`.
- `systemd/musicindex-live-relay.service`.
- `docs/adr/**`, `AGENTS.md`, `docs/README.md`.

## Constraints

- Put the mode logic in one function in `src/store.rs`, for example
  `fn make_private(path: &Path) -> Result<Option<u32>, StoreError>`:
  - When the file does not exist, make it with
    `OpenOptions::new().write(true).create_new(true).mode(0o600)`. Give
    `None`.
  - When the file exists and `mode & 0o077 != 0`, set the mode `0o600` with
    `std::fs::set_permissions`. Give `Some(previous_mode & 0o777)`.
  - When the file exists with no group or other bit, change nothing. Give
    `None`.
  - Each I/O error gives a new variant, for example
    `StoreError::Permissions { path, source: std::io::Error }`. Its
    `Display` names the path and the cause, as `StoreError::Open` does.
- `SqliteEventStore::open` calls the function before `Connection::open`.
- `SqliteEventStore::open` gives the previous mode to its caller. Choose
  the smallest change: a new public method or a field that the caller reads
  one time is acceptable. The caller in `src/lib.rs` logs one
  `tracing::warn!` that names the path and the previous mode in octal, for
  example `previous_mode = format!("{mode:o}")`.
- The store does not log. The test does not read log output.
- Do not change the umask in a test.
- The code is for Unix. Put `#[cfg(unix)]` on the mode code only if the
  crate builds on another target today. Do not add a non-Unix path.

## Implementation Steps

1. Add the error variant and its `Display`.
2. Add the mode function and call it from `SqliteEventStore::open`.
3. Give the previous mode to the caller, and log the warning in
   `src/lib.rs`.
4. Add the tests of §Acceptance Criteria.
5. In `docs/runbooks/reserved-live-items.md`, add a step after a deployment
   that reads the mode of the state file with `stat -c '%a %U %n'` and
   expects `600`. In the backup procedure, state that the reader is the
   relay user or root.
6. In `README.md`, add one sentence near `STATE_FILE`: the relay makes the
   file with the mode `0600` and corrects a wider mode at each start
   (ADR 0009).

## Acceptance Criteria

Mechanical. Each item is a test in `src/store.rs`:

- A store that opens a new path makes a file with the mode `0600`
  (`metadata.permissions().mode() & 0o777 == 0o600`).
- A file with the mode `0644` has the mode `0600` after the store opens it,
  and the store gives the previous mode `0o644`.
- A file with the mode `0600` stays `0600`, and the store gives no previous
  mode.
- A file with the mode `0640` gives the previous mode `0o640`, and a file
  with the mode `0604` gives `0o604`. This proves that a group bit and an
  other bit each start the change.
- Each existing test passes with no change to its assertions.

Manual. A test cannot make the relay fail to set the mode, because the
test process owns each file that it makes. The review reads the error path
and confirms that the error names the path.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- SQLite does not accept the empty file that the relay makes.
- An existing test must change its assertion.
- The previous mode cannot reach the caller without a change to the
  `EventStore` trait.
- The change needs a new dependency.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0009-state-file-mode.md
- docs/tasks/state-file-mode-task-001-private-state-file.md
- src/store.rs, src/lib.rs (RelayState::try_new and the callers of SqliteEventStore::open), docs/runbooks/reserved-live-items.md, README.md

Goal:
- The relay makes the state file with the mode 0600.
- At each start, the relay sets 0600 on an existing state file with a group or other bit, and logs one warning.
- When the relay cannot set the mode, the start fails with an error that names the path.

Constraints:
- Follow §Constraints of the packet exactly. Use std only. The store does not log. Do not test log output. Do not change the umask in a test.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.

Do not touch:
- the schema of the state file, the routes, Cargo.toml, systemd/, docs/adr/**, AGENTS.md, docs/README.md

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

Reviewed 2026-10-21. The full gate passes. With task 001 of the read
timeouts plan in the same tree, the gate has 220 tests.

- `make_private` in `src/store.rs` makes a new file with
  `create_new(true)` and the mode `0o600`. It corrects an existing file with
  a group or other bit, and gives the mode before the change.
- `SqliteEventStore::previous_mode` gives that mode to the caller.
  `RelayState::try_with_clock` logs one warning with the path and the mode
  in octal. The store does not log.
- Each I/O error becomes `StoreError::Permissions`, and its `Display`
  names the path. The review read this path, because a test cannot cause
  the error.
- On a target that is not Unix, `make_private` changes nothing. The relay
  is built for Linux, so this has no effect on a deployment.
- The review found no defect.

