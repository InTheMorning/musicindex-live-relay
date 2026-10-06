# ADR 0005 Display Pairing: Phase Plan

Status: Ready 2026-10-06. This plan does not make rules. ADR 0005 owns
them. The operator accepted ADR 0005 on 2026-10-06.

## Goal

A display track can carry `songLine` and `value {eventGuid, blockGuid}`. The
relay validates their shape and passes them through.

## Non-Goals

- No check that `value` names a real block.
- No change to the live value routes, to Socket.IO or to the images.

## Affected Modules

| Module | Change |
|---|---|
| `src/lib.rs`, `validate_display` | Accepts the two optional keys |
| `README.md`, `docs/interoperability.md` | The track shape |

## Sequence

1. [Task 001](../tasks/display-pairing-task-001-song-line-and-value.md): the
   two keys, their validation, their tests and the documents.

## Risk Areas

- **The deployment order.** This relay must be deployed before a publisher
  sends the keys. Publisher ADR 0012 says the same.

## Test Strategy

Tests in `tests/api.rs` for each accepted and each refused shape, and for the
pass-through on `GET /display` and `/display/events`.

## Rollback Strategy

A relay before this task refuses the new keys. Roll back the publisher of ADR
0012 first.

## Review

[ADR 0005 review checklist](../reviews/adr-0005-review-checklist.md).
