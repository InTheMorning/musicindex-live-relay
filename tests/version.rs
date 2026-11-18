//! The `--version` line of the relay.
//!
//! Owner: musicindex ADR 0083 R-VERSION-2 and R-VERSION-9 item 1. The test of
//! the info route is `info_gives_the_build_values` in `tests/api.rs`.

use std::process::Command;

#[test]
fn version_prints_the_build_values_in_one_line() {
    const FIX: &str = "musicindex ADR 0083 R-VERSION-2: `--version` prints the name, the version, \
                       the git revision and the build date in one line, and starts no server. \
                       Keep the `--version` branch in src/main.rs and keep `build.rs`.";

    let output = Command::new(env!("CARGO_BIN_EXE_musicindex-live-relay"))
        .arg("--version")
        // A bind address that cannot parse stops a relay that starts.
        .env("BIND", "not-an-address")
        .output()
        .expect("start the relay");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{FIX}\n{stdout}");

    // `build.rs` gives the two build values to each target of this package,
    // so this test reads the same values as the relay.
    let revision = option_env!("MUSICINDEX_BUILD_GIT_REVISION");
    let date = option_env!("MUSICINDEX_BUILD_BUILT_AT").map(|built_at| {
        built_at
            .get(..10)
            .expect("the build time starts with a date")
    });
    // The build time is always known: the build script reads the clock when
    // `RELAY_BUILT_AT` is not set.
    assert!(date.is_some(), "{FIX}");
    let known: Vec<&str> = [revision, date].into_iter().flatten().collect();
    let expected = format!(
        "musicindex-live-relay {} ({})\n",
        env!("CARGO_PKG_VERSION"),
        known.join(", ")
    );

    assert_eq!(stdout, expected, "{FIX}");
}
