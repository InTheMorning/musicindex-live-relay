# ADR 0004 Implementation Review

## Status

Pass - 2026-10-06. The mechanical gates and the deployment gate pass. The
visual gate is open. This review is the named artifact for the `Implemented`
status of ADR 0004 when that gate closes.

## Reviewed Artifacts

- `docs/adr/0004-listener-timeline-delay.md`
- `docs/plans/adr-0004-listener-timeline-phase-plan.md`
- `docs/tasks/listener-timeline-task-001-delay-header.md` to
  `docs/tasks/listener-timeline-task-003-expiry-delete-and-docs.md`, with the
  §Review Result of each task
- `docs/interoperability.md`
- `AGENTS.md`
- `README.md`
- `src/lib.rs` (constants, `AppConfig`, `LiveEventState`, `LiveEventInner`,
  `PendingListenerUpdate`, `parse_listener_delay_header`,
  `publish_metadata`, `release_listener_updates`, `expire_leases`,
  `delete_reserved`)
- `tests/api.rs` (listener delay header tests, listener timeline tests)

## Test Commands

All four commands are Green on 2026-10-06:

- `cargo fmt -- --check`
- `cargo build`
- `cargo test`: 47 unit tests, 116 HTTP tests and 2 startup tests
- `cargo clippy --all-targets -- -D warnings`

The three new test modules are `listener_delay_header`, `listener_timeline` and
`listener_timeline::expiry_and_delete`. Each test uses the injected clock. No
test uses a wall-clock sleep.

## Invariants

The tests are in `tests/api.rs`, module `listener_timeline` and its submodules,
unless the table names another place.

| Invariant | Result | Guard |
|---|---|---|
| The Socket.IO payload and its shape do not change. Only its time changes. | Pass | The Socket.IO emit path reads `listener_value` and emits it unchanged. The payload comes from `metadata.clone()` or `json!({})`. The JSON is never transformed. |
| The listener timeline has the updates of the instant timeline in the same order. | Pass | `a_short_delay_after_a_long_delay_is_released_in_the_order_of_the_instant_timeline`. A publish with delay 30 at T, then a publish with delay 0 at T+1, gives releases at T+30 and T+30 (the order rule). The second update waits for the first. |
| SSE, `GET /metadata` and the display routes never wait for the delay. | Pass | `a_delayed_publish_reaches_the_instant_timeline_at_once_and_the_listener_timeline_after_the_delay`. The sender broadcasts `snapshot` at once, before `apply_listener_update` and `emit_socket_remote_value`. The display routes read `display` state, which is not on the listener timeline. |
| A publish with no `Listener-Delay-Secs` header behaves as before. | Pass | `a_publish_with_no_header_is_accepted_with_delay_zero`. `GET /remoteValue` changes with no call to `release_listener_updates` when delay is 0 and no update waits. |
| No transport sends an update of a deleted event. | Pass | `delete_removes_the_item_and_a_later_read_answers_404`. The pending updates clear before the disconnect. The release sweep skips the event because `list_ids()` does not name it. |

### Code

| Point | Result | Location |
|---|---|---|
| No table, event or display lock is held across a Socket.IO emit or an SSE send | Pass | `publish_metadata` drops `inner` before `emit_socket_remote_value`. `release_listener_updates` drops `inner` before the loop that calls `emit_socket_remote_value`. `expire_leases` drops `inner` before `emit_socket_remote_value`. `delete_reserved` takes `listener_order`, writes `inner`, drops `inner`, then disconnects. |
| The lock `listener_order` is held across its emits on purpose, to keep the emit order | Pass | Each writer holds `listener_order` through its Socket.IO emit. The lock order is `listener_order`, then `inner` in every path. No path takes any lock after releasing `listener_order`. `src/lib.rs` lines 694–728 (publish), 1201–1253 (expiry), 1269–1319 (release), 608–621 (delete). |
| The pending list has its bound, and a test fills it | Pass | `a_full_pending_list_has_the_new_update_replace_the_newest_pending_update`. With `MAX_PENDING_LISTENER_UPDATES=2`, three publishes with delay 30 leave two updates and add the third by replacing the second (which has the later release time). `src/lib.rs` lines 1526–1537. |
| Each new limit has a default constant and an `AppConfig` field | Pass | `DEFAULT_MAX_LISTENER_DELAY_SECS` (300) and `DEFAULT_MAX_PENDING_LISTENER_UPDATES` (64). Both fields in `AppConfig`. Both parsed in `AppConfig::from_env`. `src/lib.rs` lines 83–86, 148–151, 171–175, 192–193. |
| No test uses a wall-clock sleep | Pass | Tests drive the clock with `state.set_now()` and call `release_listener_updates` directly. Lease tests follow the same pattern. |
| The two body forms and their separation do not change | Pass | `into_metadata` does not use the listener delay. The body handling is unchanged. The separation rule (exact key match) is in `into_metadata`. |
| `MAX_PENDING_LISTENER_UPDATES` must be 1 or more | Pass | `parse_max_pending_listener_updates` rejects a value below `MIN_PENDING_LISTENER_UPDATES` (1). The unit test `max_pending_listener_updates_of_zero_is_a_configuration_error` confirms the rejection. `src/lib.rs` lines 234–248. |

### Header Parsing

| Point | Result | Test |
|---|---|---|
| No header: the delay is 0 | Pass | `a_publish_with_no_header_is_accepted_with_delay_zero` |
| A whole number from 0 to `MAX_LISTENER_DELAY_SECS`: that delay | Pass | `a_publish_with_a_valid_header_is_accepted_and_stores_the_delay` (30) and `an_invalid_header_value_returns_400_invalid_listener_delay` (301 is rejected) |
| An empty value, a sign, a fraction, or more than one header: `400 invalid_listener_delay` | Pass | `an_invalid_header_value_returns_400_invalid_listener_delay` (tests "", "-1", "1.5", "abc", "301"). `more_than_one_header_returns_400_invalid_listener_delay`. |
| A bad header does not use a publish slot of the rate limit | Pass | `a_bad_header_does_not_use_a_publish_rate_limit_slot` |
| The header name is case-insensitive | Pass | `HeaderMap::get_all` uses lowercase lookup. The constant `LISTENER_DELAY_HEADER` is "listener-delay-secs". `src/lib.rs` lines 91–94, 2572–2579. |

### Socket.IO and GET /remoteValue

| Point | Result | Test |
|---|---|---|
| A publish with delay 30 at T gives SSE and `GET /metadata` at T, Socket.IO and `GET /remoteValue` at T+30 | Pass | `a_delayed_publish_reaches_the_instant_timeline_at_once_and_the_listener_timeline_after_the_delay` |
| A publish with no header emits at once | Pass | `a_publish_with_no_header_is_accepted_with_delay_zero` (implies no call to release function) |
| A Socket.IO client on connect gets the listener value, not the newest payload | Pass | The Socket.IO namespace handler calls `state.listener_value()` which reads `event.inner.read().await.listener_value`. `src/lib.rs` lines 2305–2327. |

### Lease Expiry

| Point | Result | Test |
|---|---|---|
| A lease expiry gives `{}` on Socket.IO after the last block | Pass | `a_lease_expiry_joins_the_listener_timeline_with_the_delay_of_the_last_publish`. A publish has delay 30 at T, and the lease expires at T+90. `GET /metadata` gives `404` at T+90. `GET /remoteValue` gives the payload until T+119 and `{}` from T+120. |
| A delay longer than the lease: `{}` arrives after the newest payload | Pass | `a_delay_longer_than_the_lease_keeps_the_order_through_the_expiry`. A publish with delay 120 at T, a second with delay 120 at T+5, lease expiry at T+95. The releases are at T+120, T+125, and T+215. The `{}` of the expiry waits for the last publish's delay. |
| Lease expiry uses the delay of the last publish, not a stored value | Pass | The relay does not store the delay across restarts. Each expiry reads the current `event.listener_delay_secs()`. `src/lib.rs` line 1234. |

### Delete

| Point | Result | Test |
|---|---|---|
| A delete with a pending update removes the updates and no release follows | Pass | `a_delete_with_a_pending_update_sends_no_release`. The pending list clears before disconnect. |
| A release sweep skips a deleted event | Pass | `list_ids()` does not name a deleted event. The sweep iterates only over existing events. |

## Reconciliation With The Other Repositories

- `musicindex-live-publisher` ADR 0011 is the sibling change. The publisher
  sends each block at once with the `Listener-Delay-Secs` header. An earlier
  publisher waits for the delay itself and sends no header, so this relay
  applies no delay to it. An earlier relay ignores the header, so a new
  publisher with an earlier relay sends each block to Socket.IO too early.
  The review checklist names this as the deployment gate: deploy the relay
  before a publisher that sends the header.
- The `v4vmm` reads `GET /metadata`, which is instant. ADR 0004 §Before
  Acceptance item 3 asks whether `v4vmm` accepts that it shows the present
  truth, not the listener timeline value. That item stays open as a request in
  `v4vmm`. It is not a gate of this review.
- `docs/interoperability.md` now names the timing of each route. The document
  matches the shipped behavior.

## Missing Tests

These points have no automatic test. None of them is an ADR 0004 invariant.

- The Socket.IO emit order under concurrent publishes, expiries, and sweeps.
  The tests have no Socket.IO client, so they cannot observe the emits
  directly. The lock structure and the unit test
  `a_publish_waits_for_a_held_listener_order_lock` are the evidence. That unit
  test fails when a publish does not take `listener_order` first.
- A podcast app that changes the block at the same time as before the change,
  with the same delay value. The review checklist names this as the visual
  gate.
- The timing under a real clock, not the injected one. The tests use
  `RelayState::with_clock` to inject a clock and drive the time with
  `set_now()`. No test measures wall-clock delays or interacts with a system
  timer.

## Drift

None. The ADR text, `README.md`, `docs/interoperability.md`, and `AGENTS.md`
all state the shipped behavior.

## Merge Recommendation

Merge when both open gates close. Each mechanical invariant has a passing test.
The code follows the ADR requirements. Lock ordering is correct and enforced by
a unit test. The tests cover edge cases.

The deployment gate passed on 2026-10-06:

On 2026-10-06 the operator deployed this relay to `api.musicindex.org`, then
publisher `r83` (commit `9dbd0a1`) with `stream_delay_secs = 15`. A poller
read `/metadata`, `/remoteValue` and `/display` of a reserved event two times
each second:

- With no delay setting, `remoteValue` changed in the same poll as
  `metadata`.
- With 15 seconds, `remoteValue` changed 15.5 s after `metadata` for a dead
  block, and 15.6 s after it for a track block.
- `display` changed in the same poll as `metadata`.

Keep the visual gate open:

- A person must examine a podcast app on Socket.IO with the broadcaster
  sending a delay.
- Confirm the app changes the block at the same time as before this ADR, with
  the same delay value from the broadcaster.
