# Proposal: A Live Event Is A Lease

Date: 2026-09-26. Status: proposal for an ADR in this repository. This plan
states no rule. The operator decided the direction on 2026-09-26, in a
discussion of `stophammer` ADR 0064.

## Problem

A live event stays "on" until something turns it off. When the broadcaster
forgets, or its software stops, nothing turns it off:

- An ephemeral event expires after the idle TTL of 24 hours. A listener that
  subscribes also calls `touch` (`src/lib.rs`, `subscribe` and the SSE
  stream). So an event with listeners never expires.
- A reserved event (ADR 0001) ignores the idle TTL.
- `remoteValue` gives the latest snapshot with no age limit. A listener can
  connect some hours after the broadcaster stopped. It then gets the payment
  destinations of the last track, and pays for music that no longer plays.

ADR 0001 already calls a stale snapshot a payment defect, but applies that
rule only after a restart.

## Proposal

The relay treats "on air" as a lease. Only the broadcaster renews it.

1. The broadcaster renews the lease with each metadata publish, and with a
   keepalive at a fixed interval while it plays. The interval is a
   configuration value, for example 30 seconds.
2. The event is on air while the last renewal is younger than the lease
   duration, for example 90 seconds.
3. When the lease expires:
   - `remoteValue` gives `{}`, so no listener pays a stale destination.
   - The SSE and Socket.IO subscribers receive an empty payload one time.
   - The event stays, so a reserved event keeps its identifier and token.
4. A listener connection keeps the event from the idle TTL, but it never
   renews the lease.
5. The metadata route gives `on_air` and `last_renewed_at`, so a client can
   tell "on air" from "known, but silent".

## What each case gives

| Case | Result |
|---|---|
| The show plays, also past its scheduled end | On air |
| The broadcaster stops the software | Off air after one lease |
| The software or the computer stops | Off air after one lease |
| A station that runs 24 hours a day | On air while it broadcasts |
| A weekly show on a reserved event | On air during each show, off air between shows |

## Consumers

- `musicindex-live-publisher` sends the keepalive from its relay worker while
  the drop file says a track plays. Its repository records that change.
- `stophammer` ADR 0064 reads `podcast:liveValue` from the RSS and gives it
  with each live row. A client then asks this relay if the event is on air.
  Stophammer does not call the relay.
- `v4vmm` shows `on_air` in its broadcast surface.

## Open Questions

1. The keepalive route: a new route, or an empty publish on the metadata
   route.
2. The lease duration and the keepalive interval, and whether a broadcaster
   can choose them within limits.
3. An explicit stop message from the broadcaster, so the lease ends at once and
   not after one lease duration.
4. A broadcaster that does not send keepalives yet. One option: a publish
   renews the lease for a longer duration during a transition period.
5. The wire contract: `on_air` and `last_renewed_at` are new fields of a public
   contract, so the change needs an ADR here first.
