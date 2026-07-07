//! Finding 13: the cloud pipeline's compile-cache probe runs `asw --version`
//! against the remote binary, but the CLI never defined a `version` clap
//! attribute, so the flag didn't exist and the probe always failed. This
//! exercises the real built binary end-to-end to confirm `--version` now
//! works, exits 0, and prints the binary name.

use std::process::Command;

#[test]
fn version_flag_exits_success_and_prints_name() {
    let output = Command::new(env!("CARGO_BIN_EXE_asw"))
        .arg("--version")
        .output()
        .expect("failed to run the asw binary");

    assert!(
        output.status.success(),
        "asw --version should exit 0, got: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("asw"),
        "expected version output to mention the binary name, got: {:?}",
        stdout
    );
}
