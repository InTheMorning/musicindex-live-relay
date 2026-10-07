# ADR 0006 Display Album: Phase Plan

Status: Ready 2026-10-06. This plan does not make rules. ADR 0006 owns them.

## Goal

A display track can carry `album`. The relay validates it and passes it
through without a change.

## Non-Goals

- No change to the live value routes, Socket.IO or the listener timeline.
- No album in the live value payload.

## Sequence

1. [Task 001](../tasks/display-album-task-001-album-key.md): the key, its
   checks and its tests.

## Schema And API

`README.md` and `docs/interoperability.md` add `album` to the display track.
An older publisher sends no `album`, so it is not affected.

## Risk Areas

- The deployment order. An older relay refuses a track with `album`. Deploy
  this relay before the publisher of `musicindex-live-publisher` ADR 0013.

## Test Strategy

Each accepted and each refused shape is a test in `tests/api.rs`.

## Rollback Strategy

Revert the commit. A publisher that sends `album` then gets
`400 invalid_display`, so stop that publisher or deploy its earlier release
first.

The review uses the
[ADR 0006 review checklist](../reviews/adr-0006-review-checklist.md).
