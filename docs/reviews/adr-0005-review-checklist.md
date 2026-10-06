# ADR 0005 Review Checklist

Status: open - 2026-10-06. Task 001 is done, and each item above
§Cross-Repository passes. The deployment item is open. ADR 0005 becomes
`Implemented` only when each item passes.

## Invariants

- [x] A track with only `artist`, `title` and `artwork` is valid, as before.
- [x] The relay never changes `songLine` or `value`.
- [x] The live value routes do not show `songLine` or `value`.

## Code And Tests

- [x] Each accepted and each refused shape of ADR 0005 has a test.
- [x] `songLine` is counted in characters, not bytes.
- [x] The existing display tests pass with no change.

## Documents

- [x] `README.md` and `docs/interoperability.md` give the track shape with
  the two optional keys.
- [x] `AGENTS.md` §Current State gives the present track shape.

## Cross-Repository

- [ ] This relay is deployed before a publisher of ADR 0012 sends the keys.
