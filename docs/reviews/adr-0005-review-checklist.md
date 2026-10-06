# ADR 0005 Review Checklist

Status: open. Use this checklist after display pairing task 001. ADR 0005
becomes `Implemented` only when each item passes.

## Invariants

- [ ] A track with only `artist`, `title` and `artwork` is valid, as before.
- [ ] The relay never changes `songLine` or `value`.
- [ ] The live value routes do not show `songLine` or `value`.

## Code And Tests

- [ ] Each accepted and each refused shape of ADR 0005 has a test.
- [ ] `songLine` is counted in characters, not bytes.
- [ ] The existing display tests pass with no change.

## Documents

- [ ] `README.md` and `docs/interoperability.md` give the track shape with
  the two optional keys.
- [ ] `AGENTS.md` §Current State gives the present track shape.

## Cross-Repository

- [ ] This relay is deployed before a publisher of ADR 0012 sends the keys.
