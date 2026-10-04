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

- [ADR 0001: Reserved live items](adr/0001-reserved-live-items.md) — a durable
  event class for stations and repeating shows
- [ADR 0002: A live event is a lease](adr/0002-live-lease.md) — Implemented. An
  event goes off air when its broadcaster stops the keepalive
- [ADR 0003: Display state and artwork](adr/0003-display-state-and-artwork.md)
  — Accepted. Optional display routes and an image store for reserved events

## Plans

- [Reserved live items phase plan](plans/adr-0001-reserved-live-items-phase-plan.md)
- [Live lease proposal](plans/live-lease-heartbeat-proposal.md) — the proposal
  that ADR 0002 answers
- [Live lease phase plan](plans/adr-0002-live-lease-phase-plan.md)
- [Display state phase plan](plans/adr-0003-display-state-phase-plan.md)

## Tasks

Packets for the reserved live items plan. Strictly sequential.

- [001 — Event store boundary](tasks/reserved-live-items-task-001-event-store-boundary.md)
- [002 — Reserved class and admin credential](tasks/reserved-live-items-task-002-reserved-class-and-admin-credential.md)
- [003 — Restore on startup and TTL exemption](tasks/reserved-live-items-task-003-restore-and-ttl-exemption.md)
- [004 — List and delete reserved items](tasks/reserved-live-items-task-004-list-and-delete.md)
- [005 — Guards, runbook, and review](tasks/reserved-live-items-task-005-guards-and-review.md)

Packets for the display state plan. ADR 0003 governs them. They need the
reserved live items packets first. Task 002 needs task 001.

- [001 — The display state and its routes](tasks/display-state-task-001-state-and-routes.md)
- [002 — The artwork store](tasks/display-state-task-002-artwork-store.md)

Packets for the live lease plan. Strictly sequential. They can go before or
after the reserved live items packets.

- [001 — Lease state and expiry](tasks/live-lease-task-001-lease-state-and-expiry.md)
- [002 — Keepalive route and documents](tasks/live-lease-task-002-keepalive-route-and-docs.md)

## Reviews

- [Live lease review checklist](reviews/live-lease-review-checklist.md)

## Research

- [Broadcaster identity options](research/broadcaster-identity-options.md) —
  seven viable credential models with evidence from the cloned prior art, and
  the availability defect that makes a decision necessary
- [Curiohoster liveValue Socket.IO examples](research/curiohoster-livevalue-socketio-examples.md)
