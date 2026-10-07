# ADR 0006 Review Checklist

Status: closed - 2026-10-06. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0006.

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

- [x] This relay is deployed before a publisher of ADR 0013 sends the key.
  Done 2026-10-06 on `api.musicindex.org`. The display state had `album` for
  a V4V track and for a track that is not V4V.
