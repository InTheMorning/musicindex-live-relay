//! Startup guards for ADR 0001 task 005.
//!
//! The review of task 003 found two defects. A corrupt state file gave a
//! restart loop with no end, and the startup error printed in the `Debug`
//! form. These tests guard both corrections.

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const UNIT: &str = include_str!("../systemd/musicindex-live-relay.service");

/// Returns the value of `key` in the section `[section]` of the unit file.
fn unit_value(section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let mut in_section = false;
    for line in UNIT.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == header;
        } else if in_section
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn seconds(value: &str) -> u64 {
    value
        .trim_end_matches('s')
        .parse()
        .unwrap_or_else(|_| panic!("`{value}` is not a count of seconds"))
}

#[test]
fn the_unit_stops_the_restart_loop_of_a_failed_start() {
    const FIX: &str = "ADR 0001 task 005: a corrupt state file fails each start. Keep \
                       `StartLimitIntervalSec` and `StartLimitBurst` in the [Unit] section of \
                       systemd/musicindex-live-relay.service, so systemd stops the restart loop \
                       and marks the unit failed.";

    let interval = unit_value("Unit", "StartLimitIntervalSec").unwrap_or_else(|| panic!("{FIX}"));
    let burst = unit_value("Unit", "StartLimitBurst").unwrap_or_else(|| panic!("{FIX}"));
    let restart_sec = unit_value("Service", "RestartSec").unwrap_or_else(|| panic!("{FIX}"));

    let interval = seconds(&interval);
    let burst: u64 = burst.parse().unwrap_or_else(|_| panic!("{FIX}"));
    assert!(
        interval > 0 && burst > 0,
        "{FIX} A zero value turns the limit off."
    );
    // The failed starts must fit in the interval, or the limit never stops
    // the loop.
    assert!(
        burst * seconds(&restart_sec) < interval,
        "{FIX} {burst} starts {restart_sec} apart do not fit in {interval} seconds."
    );
}

#[test]
fn a_corrupt_state_file_stops_the_relay_with_a_readable_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_file = dir.path().join("reserved-items.sqlite3");
    let garbage = b"not a SQLite database \x00\x01\x02".repeat(128);
    std::fs::write(&state_file, &garbage).expect("write garbage");

    let mut command = Command::new(env!("CARGO_BIN_EXE_musicindex-live-relay"));
    for key in [
        "MAX_ACTIVE_EVENTS",
        "MAX_SSE_CONNECTIONS",
        "EVENT_TTL_SECS",
        "MAX_PUBLISHES_PER_EVENT_PER_SEC",
        "MAX_CREATES_PER_SEC",
        "LEASE_SECS",
        "MAX_RESERVED_ITEMS",
    ] {
        command.env_remove(key);
    }
    let mut child = command
        .env("BIND", "127.0.0.1:0")
        .env("ADMIN_TOKEN", "test-admin-token-0123456789")
        .env("STATE_FILE", &state_file)
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the relay");

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for the relay") {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("the relay did not stop with a corrupt state file");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().expect("relay output");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!status.success(), "the exit code must be non-zero: {text}");
    assert!(
        text.contains(&format!(
            "cannot open state file {}: file is not a database",
            state_file.display()
        )),
        "the error names the path and the cause: {text}"
    );
    for debug_form in ["Open {", "source:", "SqliteFailure"] {
        assert!(
            !text.contains(debug_form),
            "the error is in the Debug form: {text}"
        );
    }
    assert_eq!(std::fs::read(&state_file).expect("read"), garbage);
}
