//! The ADR link guard runs in the gate.
//!
//! Owner: the project-baseline skill, section "A Binding Rule Lives In A
//! Decision Record". The skill script examines the link lines of each ADR and
//! the status rule of `AGENTS.md`. With `--bar-from`, it also requires the
//! `- Bar:` line in each current ADR from that number. This test starts the
//! script with no `--skip`, so `cargo test` runs the guard at each gate.

use std::{path::PathBuf, process::Command};

/// The first ADR number that must have a `- Bar:` line. `AGENTS.md` gives the
/// same number.
const BAR_FROM: &str = "0010";

#[test]
fn adr_links_pass_the_project_baseline_guard() {
    let home = std::env::var_os("HOME").expect("HOME must be set to find the skill script");
    let script =
        PathBuf::from(home).join(".agents/skills/project-baseline/scripts/check_adr_links.py");
    assert!(
        script.is_file(),
        "project-baseline §A Binding Rule Lives In A Decision Record: the guard script \
         {} is missing. Install the shared skills in ~/.agents/skills.",
        script.display()
    );

    let output = Command::new("python3")
        .arg(&script)
        .args(["--bar-from", BAR_FROM])
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("python3 must start");

    assert!(
        output.status.success(),
        "project-baseline §A Binding Rule Lives In A Decision Record: check_adr_links.py \
         failed with {}. Correct the ADR link lines, the `- Bar:` line or the AGENTS.md \
         status rule that it names.\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
