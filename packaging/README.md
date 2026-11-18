# Packages

ADR 0010 owns these files. musicindex ADR 0084 owns the set of packages.

Run each command in a checkout of this repository. Each package gets its
version from `Cargo.toml`.

## Arch Package

```sh
cd packaging/arch
makepkg -f
```

The result is `packaging/arch/musicindex-live-relay-VERSION-1-x86_64.pkg.tar.zst`.

## Debian Package

Install the tool and the target one time:

```sh
cargo install cargo-deb --locked
rustup target add x86_64-unknown-linux-musl
```

Build the package:

```sh
RELAY_BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" cargo deb --target x86_64-unknown-linux-musl
```

The result is `target/debian/musicindex-live-relay_VERSION-1_amd64.deb`.

## Docker Image

```sh
docker build --target musicindex-live-relay --build-arg RELAY_GIT_REVISION="$(git rev-parse --short HEAD)" --build-arg RELAY_BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" -t musicindex-live-relay:local .
```

Read the build values of the image:

```sh
docker run --rm musicindex-live-relay:local musicindex-live-relay --version
```

Mount a volume at `/var/lib/musicindex-live-relay` to keep the reserved live
items. Publish port 8018 only on the loopback address of the host, because
`POST /v1/liveitems` is public.

## After An Install

The Arch package and the `.deb` install the unit
`musicindex-live-relay.service` and the system user `musicindex-live-relay`.
They do not enable, start or restart the unit.

```sh
sudo systemctl enable --now musicindex-live-relay.service
```

After an upgrade, restart the service when no broadcaster is on air. A
restart removes each ephemeral live item and its broadcaster token.
