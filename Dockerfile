# The image of musicindex-live-relay (relay ADR 0010, musicindex ADR 0084
# R-PACKAGE-2). The build context is the root of this repository.

FROM rust:alpine AS builder

# rusqlite has the feature "bundled", so the build compiles sqlite3.c.
RUN apk add --no-cache build-base

WORKDIR /build
COPY . .

# The build context has no Git folder. build.rs reads these two values for
# the info route and the --version line (musicindex ADR 0083 R-VERSION-8).
ARG RELAY_GIT_REVISION
ARG RELAY_BUILT_AT
ENV RELAY_GIT_REVISION=$RELAY_GIT_REVISION \
    RELAY_BUILT_AT=$RELAY_BUILT_AT
RUN cargo build --release --locked

FROM alpine:3.24@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS musicindex-live-relay

# The relay user owns the state directory. Mount a volume there to keep the
# reserved live items (ADR 0001).
RUN addgroup -S relay \
 && adduser -S -G relay relay \
 && install -d -o relay -g relay -m 0700 /var/lib/musicindex-live-relay

COPY --from=builder /build/target/release/musicindex-live-relay /usr/local/bin/musicindex-live-relay

USER relay

# The relay listens on each interface in the container. Publish the port only
# on the loopback address of the host, because POST /v1/liveitems is public.
ENV BIND=0.0.0.0:8018

EXPOSE 8018

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=12 \
    CMD wget -qO- http://127.0.0.1:8018/health || exit 1

CMD ["musicindex-live-relay"]
