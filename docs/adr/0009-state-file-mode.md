# ADR 0009: The State File Is Private To The Relay User

## Status

Accepted 2026-10-21 by the operator. The mode of the state file on
`api.musicindex.org` is recorded in §Before Acceptance.

Proposed 2026-10-21.

Class: durable. AGENTS.md §4 makes the token hashes a secret of the relay.
This record makes the file that holds them private to the relay user.

## Context

ADR 0001 stores the identity and the broadcaster token hash of each reserved
event in the SQLite state file `STATE_FILE`. ADR 0001 puts the file in the
systemd `StateDirectory`, and the unit sets `StateDirectoryMode=0700`.

A security test on 2026-10-21 started the relay with a new `STATE_FILE`. The
relay made the file with the mode `0644`. `SqliteEventStore::open` in
`src/store.rs` calls `Connection::open`, and SQLite uses the process umask.
Thus each user on the host can read the file when its directory permits it.

The risk is low:

- The relay makes each broadcaster token from 32 random bytes. A person who
  reads a SHA-256 hash of such a token cannot find the token.
- With the systemd unit, the directory has the mode `0700`.

But the directory is the only protection, and the relay does not control it:

- On `api.musicindex.org` the relay runs in Docker Compose. The mode of the
  volume directory comes from the deployment, not from the unit.
- `README.md` §Deployment permits other deployments.

A subsequent identity decision can keep other secrets in this file. The open
research is in `docs/research/broadcaster-identity-options.md`.

This record does not amend ADR 0001. ADR 0001 has the status `Implemented`,
and a test enforces each of its invariants. An amendment would add an
invariant with no test to an implemented record.

## Decision

- When the relay makes the state file, the file gets the mode `0600`. The
  relay sets the mode when it makes the file, not after SQLite opens it.
- At each start, the relay examines the mode of an existing state file. When
  the mode gives a permission to the group or to other users, the relay sets
  `0600` and logs one `warn` line. The line names the path and the mode
  before the change.
- When the relay cannot set the mode, the start fails with an error that
  names the path. This is the same result as an unreadable state file
  (ADR 0001).
- The relay does not change the mode of the parent directory. The deployment
  owns the directory.
- The rule applies on Unix. The relay is built for Linux.

SQLite on Unix gives a rollback journal the permissions of the database file.
The store sets no journal mode, so the journal is a rollback journal and gets
`0600` too.

## Invariants

- A state file that the relay makes has the mode `0600`.
- After a start, the state file gives no permission to the group or to other
  users.
- A start that cannot make the state file private fails and names the path.

## Non-Goals

- Encryption of the state file.
- A change of the content of the state file.
- A process-wide umask.

## Before Acceptance

- The operator records the mode of the Docker volume directory on
  `api.musicindex.org`. Done on 2026-10-21, for the container
  `stophammer-live-relay-1`:
  - The volume directory
    `/var/lib/docker/volumes/stophammer_live-relay-state/_data` has the mode
    `700`.
  - The state file `reserved-items.sqlite3` in it has the mode `644`.
  - The owner is the user of the container, which has no name on the host.

  The directory gives the same protection as the systemd unit. The file
  itself is readable by each user that has access to the directory. On this
  host, this record is thus a second layer.

## Verification After Implementation

Mechanical. A test proves each item:

- With the umask `022`, a new state file has the mode `0600`.
- A state file with the mode `0644` has the mode `0600` after a start, and
  the start logs one warning.
- A state file with the mode `0600` causes no warning.

Manual. A test cannot make the relay fail to set the mode. A test process
owns each file that it makes, and the owner can always set the mode. The review
of the implementation reads the error path and confirms that it names the
path.

Manual. After the deployment on `api.musicindex.org`, the operator records
the mode of the state file. The runbook `docs/runbooks/reserved-live-items.md`
gets the check.

## Alternatives Considered

### Stop At A Mode That Is Too Wide

Rejected. The relay can correct the mode itself. A stop puts the relay off
the air for a condition that it can repair.

### Leave The Mode To The Deployment

Rejected. The systemd unit protects the directory, but a Docker volume or a
manual deployment does not. The relay owns the secret, so the relay protects
the file.

### Set The Process Umask

Rejected. A umask applies to each file of the process, and a reader of
`src/store.rs` does not see it.

## Consequences

- The token hashes are private to the relay user in each deployment.
- An operator who reads the state file as another user, for a backup, must
  use the relay user or root. The runbook backup procedure states the user.
- An existing deployment gets the correct mode at its next start, and the
  log shows the change one time.

## References

- AGENTS.md §4
- ADR 0001
- `docs/runbooks/reserved-live-items.md`
- `docs/research/broadcaster-identity-options.md`
