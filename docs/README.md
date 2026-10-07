# Documentation

## Order Of Work

The packets here belong to a system that spans three repositories. The
cross-repository order lives in `v4vmm`:
`docs/plans/broadcast-chain-delivery-order.md`.

This repository blocks nothing and is blocked by nothing. Its urgency is a
product question: reserved live items matter as soon as a show repeats or a
station runs continuously.

## Architecture

- [Interoperability](interoperability.md) — what consumers depend on, the limits
  they work around, and the requested work

## ADRs

- [ADR 0001: Reserved live items](adr/0001-reserved-live-items.md) —
  Implemented. A durable event class for stations and repeating shows
- [ADR 0002: A live event is a lease](adr/0002-live-lease.md) — Implemented. An
  event goes off air when its broadcaster stops the keepalive
- [ADR 0003: Display state and artwork](adr/0003-display-state-and-artwork.md)
  — Implemented. Optional display routes and an image store for reserved
  events
- [ADR 0004: A delayed listener timeline for Socket.IO](adr/0004-listener-timeline-delay.md)
  — Accepted. The broadcaster sends the delay with each publish. Socket.IO and
  `GET /remoteValue` wait for it. SSE and the other reads are instant
- [ADR 0005: The display state carries the song line and the value identity](adr/0005-display-song-line-and-value.md)
  — Implemented. Two optional track keys, `songLine` and `value`, for the HLS
  tagger of publisher ADR 0009
- [ADR 0006: The display track carries the album](adr/0006-display-album.md)
  — Implemented. One optional track key, `album`, so an app shows the album of
  each track

## Plans

- [Reserved live items phase plan](plans/adr-0001-reserved-live-items-phase-plan.md) — Implemented
- [Live lease proposal](plans/live-lease-heartbeat-proposal.md) — the proposal
  that ADR 0002 answers
- [Live lease phase plan](plans/adr-0002-live-lease-phase-plan.md)
- [Display state phase plan](plans/adr-0003-display-state-phase-plan.md)
- [Listener timeline phase plan](plans/adr-0004-listener-timeline-phase-plan.md)
  — Ready. Three packets
- [Display pairing phase plan](plans/adr-0005-display-pairing-phase-plan.md)
  — Ready. One packet
- [Display album phase plan](plans/adr-0006-display-album-phase-plan.md)
  — Ready. One packet

## Tasks

Packets for the reserved live items plan. Strictly sequential. All five are
done, and ADR 0001 is Implemented.

- [001 — Event store boundary](tasks/reserved-live-items-task-001-event-store-boundary.md)
- [002 — Reserved class and admin credential](tasks/reserved-live-items-task-002-reserved-class-and-admin-credential.md)
- [003 — Restore on startup and TTL exemption](tasks/reserved-live-items-task-003-restore-and-ttl-exemption.md)
- [004 — List and delete reserved items](tasks/reserved-live-items-task-004-list-and-delete.md)
- [005 — Guards, runbook, and review](tasks/reserved-live-items-task-005-guards-and-review.md)

Packets for the display state plan. ADR 0003 governs them. They need the
reserved live items packets first. Task 002 needs task 001.

- [001 — The display state and its routes](tasks/display-state-task-001-state-and-routes.md)
  — Implemented - 2026-10-04
- [002 — The artwork store](tasks/display-state-task-002-artwork-store.md)
  — Implemented - 2026-10-04

Packets for the live lease plan. Strictly sequential. They can go before or
after the reserved live items packets.

- [001 — Lease state and expiry](tasks/live-lease-task-001-lease-state-and-expiry.md)
- [002 — Keepalive route and documents](tasks/live-lease-task-002-keepalive-route-and-docs.md)

Packets for the listener timeline plan. ADR 0004 governs them. Strictly
sequential. Ready - 2026-10-06.

- [001 — The delay header](tasks/listener-timeline-task-001-delay-header.md)
- [002 — The listener timeline](tasks/listener-timeline-task-002-listener-timeline.md)
- [003 — Expiry, delete and documents](tasks/listener-timeline-task-003-expiry-delete-and-docs.md)

Packet for the display pairing plan. ADR 0005 governs it. Ready -
2026-10-06.

- [001 — The song line and the value identity](tasks/display-pairing-task-001-song-line-and-value.md)
  — Implemented - 2026-10-06

Packet for the display album plan. ADR 0006 governs it. Ready - 2026-10-06.

- [001 — The album key](tasks/display-album-task-001-album-key.md)
  — Implemented - 2026-10-06

## Runbooks

- [Reserved live items](runbooks/reserved-live-items.md) — reserve, back up,
  rotate the admin token, restart, delete, and recover a corrupt state file

## Reviews

- [ADR 0001 implementation review](reviews/adr-0001-implementation-review.md)
  — the named artifact for the `Implemented` status of ADR 0001
- [ADR 0003 implementation review](reviews/adr-0003-implementation-review.md)
  — the named artifact for the `Implemented` status of ADR 0003
- [Live lease review checklist](reviews/live-lease-review-checklist.md)
- [ADR 0004 implementation review](reviews/adr-0004-implementation-review.md)
  — pass for the mechanical items and the deployment. The visual gate is
  open
- [ADR 0004 review checklist](reviews/adr-0004-review-checklist.md) — open.
  The visual item remains
- [ADR 0005 review checklist](reviews/adr-0005-review-checklist.md) — the
  named artifact for the `Implemented` status of ADR 0005
- [ADR 0006 review checklist](reviews/adr-0006-review-checklist.md) — the
  named artifact for the `Implemented` status of ADR 0006

## Research

- [Broadcaster identity options](research/broadcaster-identity-options.md) —
  seven viable credential models with evidence from the cloned prior art, and
  the availability defect that makes a decision necessary
- [Curiohoster liveValue Socket.IO examples](research/curiohoster-livevalue-socketio-examples.md)
  — the client sequence, three examples from 2026-05-15, and a capture of the
  model server on 2026-10-06
