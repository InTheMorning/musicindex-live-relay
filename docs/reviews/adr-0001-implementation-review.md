# ADR 0001 Implementation Review

## Status

Pass - 2026-10-04. Each invariant of ADR 0001 has a test. No gate is open.
This review is the named artifact for the `Implemented` status of ADR 0001.

## Reviewed Artifacts

- `docs/adr/0001-reserved-live-items.md`
- `docs/plans/adr-0001-reserved-live-items-phase-plan.md`
- `docs/tasks/reserved-live-items-task-001-event-store-boundary.md` to
  `docs/tasks/reserved-live-items-task-005-guards-and-review.md`, with the
  §Review Result of tasks 001 to 004
- `docs/interoperability.md`
- `README.md`
- `src/lib.rs`, `src/store.rs` and `src/main.rs`
- `tests/api.rs` and `tests/startup.rs`
- `systemd/musicindex-live-relay.service`
- `v4vmm`: `docs/plans/broadcast-chain-delivery-order.md` and
  `docs/architecture/broadcast-chain.md`
- `musicindex-live-publisher`: `docs/architecture/broadcast-chain-boundaries.md`

## Test Commands

All four commands are Green on 2026-10-04:

- `cargo fmt -- --check`
- `cargo build`
- `cargo test`: 41 unit tests, 62 HTTP tests and 2 startup tests
- `cargo clippy --all-targets -- -D warnings`

Each new guard was broken on purpose, and a test failed each time. These
changes broke the guards:

- the `Debug` form for the startup error
- no start limit in the unit
- a `==` comparison in `AdminToken::matches`
- a reaper that removes a reserved item
- a delete route with no admin check
- a payload write to the state directory

## Invariants

The tests are in `tests/api.rs`, module `reserved::adr_0001_invariants`,
unless the table names another place.

| Invariant | Result | Guard |
|---|---|---|
| Ephemeral behavior does not change | Pass | `ephemeral_behavior_does_not_change_with_a_state_file`. This task changes none of the 22 tests before `mod reserved` |
| No payload, snapshot, or replay buffer reaches disk | Pass | `no_payload_snapshot_or_replay_buffer_reaches_disk` |
| The stored value for a token is a hash | Pass | `the_state_file_holds_the_token_hash_and_never_the_token` |
| A durable write needs the admin credential | Pass | `each_durable_write_needs_the_admin_credential` |
| The reaper never removes a reserved item | Pass | `the_reaper_never_removes_a_reserved_item` |
| A `404` still means the event does not exist, for both classes | Pass | `event_not_found_means_the_event_does_not_exist_for_both_classes` |
| Token comparison stays constant time, for both tokens | Pass | `src/lib.rs`: `both_token_checks_use_the_constant_time_comparison` and `admin_token_matches_only_the_same_token` |

Notes on the guards:

- The no-payload test publishes to a reserved item and to an ephemeral item,
  sends a keepalive, and lets the lease expire. Then it compares each byte of
  each file in the state directory with the bytes after the reserve. No byte
  changes, and no file is added. The schema holds only `live_items` and
  `schema_version`.
- The token test compares the stored hash with the SHA-256 hash of the
  broadcaster token. No file in the state directory holds the broadcaster
  token, the admin token, or the hash of the admin token.
- The credential test sends the reserve route and the delete route with no
  credential, a wrong admin token, and two broadcaster tokens. No byte of the
  state directory changes. The public routes also change no byte.
- The reaper test uses a restored item, an item with no publish, and an item
  with an expired lease. Ten years of idle time remove only the ephemeral
  item.
- The `404` test sends five routes: the metadata read, `remoteValue`,
  `events`, a publish and a keepalive. An absent, a deleted, a reaped, and a
  restarted ephemeral identifier each get `404 event_not_found` on each route.
  An existing item of each class never gets that answer.

### Constant Time Is Proved By Inspection

A test cannot measure the time of a comparison reliably. The guard
`both_token_checks_use_the_constant_time_comparison` reads the source instead.
It proves these facts:

- `AdminToken::matches` and `StoredEvent::token_hash_matches` call
  `subtle::ConstantTimeEq::ct_eq`.
- The admin check calls `AdminToken::matches`. The publish check and the
  keepalive check call `StoredEvent::token_hash_matches`.
- No production line in `src/lib.rs` or `src/store.rs` compares a token or a
  hash with `==` or `!=`.

The guard does not prove that `subtle` itself runs in constant time. This
review accepts `subtle` for that property.

## Changes In Task 005

The review of task 003 added two behavior changes to the task 005 packet.
Both are done, and each has a guard in `tests/startup.rs`.

- `systemd/musicindex-live-relay.service` has `StartLimitIntervalSec=300` and
  `StartLimitBurst=5` in `[Unit]`. These are the values of the publisher
  units. With `RestartSec=5s`, five failed starts take about 25 seconds, so
  the limit stops the loop and systemd marks the unit `failed`.
- `src/main.rs` logs a startup error with `Display` in the line
  `relay stopped with an error`, and exits with code 1. For a corrupt state
  file, the line names the path and the cause:
  `cannot open state file <path>: file is not a database`. Before this
  change, Rust printed the `Debug` form of the error.

No other behavior changed.

## Missing Tests

These points have no automatic test. None of them is an ADR 0001 invariant.

- The Socket.IO answers: `{}` after a restart, and the disconnect after a
  delete. The tests have no Socket.IO client. Task 004 recorded this.
- The start limit under a real systemd. The guard reads the unit file only.
  `systemd-analyze verify` accepts the unit. It reports only that the binary
  `/usr/local/bin/musicindex-live-relay` is not installed on the review host.

## Drift

- ADR 0001 §A Reserved Item Stores No Payload says that a restored item
  "serves `{}`". The shipped relay gives `{}` on `remoteValue` and Socket.IO,
  and `404 metadata_not_found` on the metadata read. The review of task 003
  accepted this, because `{}` on the metadata read changes the wire format.
  `README.md`, `docs/interoperability.md`, `AGENTS.md` §Current State and the
  runbook state the shipped behavior. The ADR text and `AGENTS.md` §6 still
  say "serves `{}`". An in-place amendment of the ADR can clarify this,
  because it does not reverse the decision.
- The plan named a future "ADR 0002" for the broadcaster identity model. ADR
  0002 is now the live lease. The plan now says "a later ADR".
- Two neighbor documents still describe a relay with memory state only and no
  list or delete route. They are `v4vmm`
  `docs/architecture/broadcast-chain.md` §Known Limits and
  `musicindex-live-publisher` `docs/architecture/broadcast-chain-boundaries.md`
  §Known Limits. They are correct for an ephemeral item, but they do not
  name the reserved class. This review did not change them.
- The `v4vmm` delivery order calls this repository `splitkit`.

## Reconciliation With The Other Repositories

- The reserve response holds `event_id`, `broadcaster_token`, `metadata_url`
  and `events_url`. `v4vmm` parses these four fields into
  `LiveItemCreateResponse`. The test
  `reserve_with_correct_credential_returns_201_and_the_contract_fields` pins
  the full key set.
- `v4vmm` `docs/plans/broadcast-chain-delivery-order.md` records that the
  reserved class exists in the relay, and that `v4vmm` has no packet yet to
  reserve an item from the app.
- `docs/interoperability.md` matches the shipped behavior.

## Merge Recommendation

Merge. Every invariant has a passing test, the two behavior changes are the
two that the packet allows, and `Cargo.lock` has no change.

Follow-up work, with no gate on this ADR:

- Change the two neighbor documents in their own repositories.
- Amend the ADR 0001 text and `AGENTS.md` §6 for the metadata read after a
  restart.
- A `v4vmm` packet to reserve an item from the app.
- The follow-up work of ADR 0001: a token rotation route, metrics, and
  shared state for more than one relay.
