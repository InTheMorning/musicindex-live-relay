# ADR 0009 State File Mode: Review Checklist

Status: open - 2026-10-21. The code and document items pass. The deployment
item and the status change are open. This checklist becomes the named
artifact for the `Implemented` status of ADR 0009 when each item passes.

## Invariants Of ADR 0009

- [x] A state file that the relay makes has the mode `0600`.
- [x] After a start, the state file gives no permission to the group or to
  other users.
- [x] A start that cannot make the state file private fails and names the
  path.

## Code

- [x] The relay sets the mode before `Connection::open`, not after.
- [x] The store does not log. The caller logs one warning with the path and
  the previous mode.
- [x] The error path names the path. A reader confirms this, because a test
  cannot cause the error.
- [x] No test changes the umask.
- [x] `Cargo.toml` has no new dependency.

## Documents

- [x] `docs/runbooks/reserved-live-items.md` has the mode check after a
  deployment, and states the user for a backup.
- [x] `README.md` states the mode of the state file.
- [ ] ADR 0009 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

- [ ] After the deployment on `api.musicindex.org`, the operator records
  the mode of `reserved-items.sqlite3`. The expected mode is `600`, and the
  relay log has one warning from the first start.
