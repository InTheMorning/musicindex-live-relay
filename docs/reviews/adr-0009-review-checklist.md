# ADR 0009 State File Mode: Review Checklist

Status: closed - 2026-10-21. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0009.

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
- [x] ADR 0009 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

- [x] After the deployment on `api.musicindex.org`, the operator records
  the mode of `reserved-items.sqlite3`. The expected mode is `600`, and the
  relay log has one warning from the first start.

## Deployment Result

The operator deployed commit `c22f647` on `api.musicindex.org` on
2026-10-21 and recorded these results:

- `stat` gives `600 UNKNOWN` for
  `/var/lib/docker/volumes/stophammer_live-relay-state/_data/reserved-items.sqlite3`.
  Before the deployment, the mode was `644`.
- The first start logged one warning at 2026-10-21T20:09:46Z:
  `corrected the mode of the state file` with `previous_mode="644"`. The
  relay then started and logged `listening`.

