# ADR 0005 Review Checklist

Status: closed - 2026-10-06. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0005.

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

- [x] This relay is deployed before a publisher of ADR 0012 sends the keys.
  Done 2026-10-06. On `api.musicindex.org`, publisher `r83` sent a display
  state with `songLine` and no `value` before its payload. In the next poll,
  it sent the same state again with the `value` of that payload. The relay
  accepted both.
