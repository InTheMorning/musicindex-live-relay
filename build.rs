//! The build script gives the git revision and the build time to the compiler.
//!
//! Owner: musicindex ADR 0083 R-VERSION-8. The input variables are
//! `RELAY_GIT_REVISION` and `RELAY_BUILT_AT`. When a variable is not set, the
//! crate `musicindex-build-info` reads Git and the clock.

fn main() {
    musicindex_build_info::build::emit("RELAY");
}
