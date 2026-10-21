# ADR 0009 State File Mode: Phase Plan

Status: Packet done 2026-10-21. The deployment check is open. This plan does not make rules. ADR 0009 owns them.

## Goal

The relay makes the state file with the mode `0600`. At each start it
corrects an existing state file that gives a permission to the group or to
other users, and it logs one warning. A start that cannot set the mode
fails and names the path.

## Non-Goals

- No encryption and no change of the content of the state file.
- No change of the mode of the parent directory.
- No process-wide umask.

## Assumptions

- SQLite opens an empty file as a new database. The relay can thus make the
  file with `std::fs::OpenOptions` and the mode `0o600` before
  `Connection::open`.
- `std::os::unix::fs::OpenOptionsExt::mode` and
  `std::os::unix::fs::PermissionsExt` are sufficient. No new dependency is
  necessary.
- SQLite on Unix gives a rollback journal the permissions of the database
  file (ADR 0009 §Decision).
- The umask removes bits only. A file made with `0o600` thus has the mode
  `0600` with each usual umask, and a test does not change the umask.

## Affected Modules

| Module | Change |
|---|---|
| `src/store.rs` | `SqliteEventStore::open` makes the file private before SQLite opens it. A new `StoreError` variant for a mode that the relay cannot set. |
| `src/lib.rs` | The caller of `SqliteEventStore::open` logs the warning, if the store gives the mode before the change. |
| `docs/runbooks/reserved-live-items.md` | The mode check after a deployment, and the user for a backup |
| `README.md` | One sentence on the mode of the state file |

## Sequence

1. [Task 001](../tasks/state-file-mode-task-001-private-state-file.md): the
   private state file, the warning, the error, and the documents.

One packet. It can ship by itself.

## Schema And API

No change of a route, a body, a status code or the schema of the state file.

## Risk Areas

- **A start that stops.** A state file that the relay cannot change stops the
  start. On each known deployment the relay user owns the file, so the
  change works. The review reads the error path.
- **A test that changes the umask.** The umask belongs to the full test
  process, and the tests run in parallel. No test changes it.

## Test Strategy

Unit tests in `src/store.rs` with a temporary directory. They read the mode
with `std::fs::metadata`. The test of the warning tests the value that the
store gives, not the log output.

## Rollback Strategy

Revert the commit. The relay can open a state file with the mode `0600`
after a revert, so no data changes are necessary.

The review uses the
[ADR 0009 review checklist](../reviews/adr-0009-review-checklist.md).
