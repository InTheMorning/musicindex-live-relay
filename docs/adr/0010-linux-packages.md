# ADR 0010: The Linux Packages Of The Relay

## Status

Proposed - 2026-11-18.

Proposed 2026-11-18. The orchestrator session relayed the task of
musicindex ADR 0084, which the operator accepted on 2026-11-18. The operator
has not accepted the selections of this record.

- Bar: difficult to change. A package puts the binary, the unit and a system
  user on each host that installs it. A subsequent change of a path or of the
  user name needs a migration on each of those hosts.

Class: situational. musicindex ADR 0084 owns the set of packages, and each of
its rules is situational. A record that supersedes ADR 0084 can remove this
record.

## Context

musicindex ADR 0084 rule R-PACKAGE-2 gives the relay three packages: an Arch
PKGBUILD, a Debian `.deb` and a Docker image. The PKGBUILD and the `.deb`
install a systemd unit. Rule R-PACKAGE-5 tells each repository to select and
own the workflow that builds its packages. This record does not write those
rules again.

Terms:

- A **PKGBUILD** is the build script of an Arch Linux package. This is an
  established Arch term.
- **cargo-deb** is a Cargo command that makes a `.deb` from the metadata in
  `Cargo.toml`. This is an established tool name.
- A **static binary** contains each library that it needs. This is an
  established term.

The conditions on 2026-11-18:

- The repository had the unit `systemd/musicindex-live-relay.service` and no
  package. The unit started `/usr/local/bin/musicindex-live-relay` as root.
- On `api.musicindex.org` the relay runs in Docker Compose.
  `../stophammer/deploy-live-relay.sh` builds a static binary and makes the
  image from `../stophammer/Dockerfile.deploy`. That deploy does not use the
  unit.
- The repository has no CI workflow.
- A restart of the relay removes each ephemeral live item and its
  broadcaster token (ADR 0001).

## Decision

### The Files

| Package | Files | Tool |
| --- | --- | --- |
| Arch | `packaging/arch/PKGBUILD`, `packaging/arch/musicindex-live-relay.install` | `makepkg` |
| Debian | `[package.metadata.deb]` in `Cargo.toml`, the scripts in `packaging/debian/` | cargo-deb |
| Docker | `Dockerfile`, `.dockerignore` | `docker build` |

`packaging/README.md` gives the command for each package.

### The Unit And Its User

- The Arch package and the `.deb` install the binary as
  `/usr/bin/musicindex-live-relay`. They install the one unit file
  `systemd/musicindex-live-relay.service` into `/usr/lib/systemd/system/`.
- The unit runs the relay as the system user `musicindex-live-relay`. The
  file `packaging/sysusers.d/musicindex-live-relay.conf` makes the user. The
  two packages install it into `/usr/lib/sysusers.d/`.
- The unit keeps `StateDirectory=musicindex-live-relay`. systemd makes
  `/var/lib/musicindex-live-relay` for the user of the unit, so the packages
  have no `tmpfiles.d` file.
- The packages install no file in `/etc`. The runbook
  `docs/runbooks/reserved-live-items.md` makes the admin environment file and
  the drop-in.

### A Package Does Not Start Or Restart The Relay

- An install does not enable and does not start the unit. The operator does
  that.
- An upgrade does not restart the unit, because a restart removes each
  ephemeral live item. The package prints a message that tells the operator
  to restart the service.
- A removal of the `.deb` stops and disables the unit. The state directory
  and the system user stay, because the state file holds the reserved live
  items.

### The Arch Package

- `makepkg` builds the checkout that holds the PKGBUILD. It downloads no
  source.
- `pkgver` comes from the version in `Cargo.toml`. The stophammer PKGBUILD
  had `pkgver=0.8.0` while its `Cargo.toml` had `0.21.1`. A value that the
  PKGBUILD reads from `Cargo.toml` cannot be different.
- The PKGBUILD has no `check()` function. The gate of the repository runs
  the tests. The test `tests/adr_links.rs` needs a skill script that is only
  on a host of the operator.

### The Debian Package

- The command is `cargo deb --target x86_64-unknown-linux-musl`. The binary
  is static. Thus a build host that is not Debian can make the package, and
  the package depends only on `systemd`.
- The `postinst` script runs `systemd-sysusers` for the user file and
  `systemctl daemon-reload`.

### The Docker Image

- The `Dockerfile` builds the relay in a `rust:alpine` stage and copies the
  binary into the Alpine image that `../stophammer/Dockerfile.deploy` uses.
- The image has the user `relay` and the folder
  `/var/lib/musicindex-live-relay` with the mode `0700`. These are the names
  of `../stophammer/Dockerfile.deploy`, so a state volume of that deploy
  fits this image.
- The build context has no `.git` folder. The build arguments
  `RELAY_GIT_REVISION` and `RELAY_BUILT_AT` give the build values (musicindex
  ADR 0083 R-VERSION-8).
- This record does not change the deploy on `api.musicindex.org`.

### The Workflow

The workflow is local. A person or an agent runs the three commands of
`packaging/README.md` in a checkout. This work adds no CI workflow.

## Open

1. A CI release workflow. The mechanical check of musicindex ADR 0084 names
   a release workflow that makes each package. The repository has no CI, and
   the operator does each tag, push and release. The decision needs the
   registry for the image and the place for the two package files.
2. If the deploy on `api.musicindex.org` changes to the `Dockerfile` of this
   repository. That is a decision for stophammer, which owns the deploy.

## Checks

Mechanical. The record of each run is in the commit message and in the report
to the orchestrator session. No test runs these builds, because the builds
need tools that the gate does not have.

- `makepkg` makes the Arch package. The package contains the binary, the
  unit and the user file.
- `cargo deb --target x86_64-unknown-linux-musl` makes the `.deb`. The
  package contains the binary, the unit and the user file.
- `docker build` makes the image, and `--version` in the image prints the
  two build arguments.

For a person, on a host with systemd:

- After an install, `systemctl start musicindex-live-relay` starts the relay
  as the user `musicindex-live-relay`, and
  `/var/lib/musicindex-live-relay` belongs to that user.

## Alternatives Considered

### A `.deb` From `dpkg-deb` And A `debian/` Folder

Rejected. That build needs a Debian host or a Debian container, and it needs
more files. cargo-deb reads the metadata of `Cargo.toml` and operates on each
host that has Cargo.

### `DynamicUser=yes` In The Unit

Rejected. systemd then makes a temporary user and puts the state below
`/var/lib/private`. The runbook reads the owner and the mode of the state
file, and a fixed user keeps those steps simple.

### Two Unit Files, One For A Package And One For `/usr/local/bin`

Rejected. Two files can differ. An install by hand copies the binary to
`/usr/bin` and uses the same unit.

## Consequences

- The unit does not start the relay as root. An install by hand must make
  the user with the file in `packaging/sysusers.d/` before the first start.
- When the unit started as root before, systemd gives the state directory to
  the new user at the next start.
- A release has three more commands. `packaging/README.md` gives them.

## References

- musicindex ADR 0084, `../musicindex/docs/adr/0084-the-linux-packages-of-each-suite-app.md`
- musicindex ADR 0083, rule R-VERSION-8, for the build values
- ADR 0001, reserved live items, for the state file
- ADR 0009, the mode of the state file
- `../stophammer/packaging/` and `../stophammer/Dockerfile`, the models
