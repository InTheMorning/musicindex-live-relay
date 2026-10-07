# ADR 0006 Review Checklist

Status: open - 2026-10-06. Task 001 is done, and each item above
§Cross-Repository passes. The deployment item is open. ADR 0006 becomes
`Implemented` only when each item passes.

## Invariants

- [x] A track with no `album` key is valid, as before.
- [x] The relay never changes `album`.
- [x] The live value routes do not show `album`.

## Code And Tests

- [x] Each accepted and each refused shape of ADR 0006 has a test.
- [x] `album` is counted in characters, not bytes.
- [x] The existing display tests pass with no change.

## Documents

- [x] `README.md` and `docs/interoperability.md` give the track shape with
  `album`.
- [x] `AGENTS.md` §Current State gives the present track shape.

## Cross-Repository

- [ ] This relay is deployed before a publisher of ADR 0013 sends the key.
