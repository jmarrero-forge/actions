//! The zizmor recipe runs the online audits when GH_TOKEN is set and falls
//! back to --offline, with a note, when it is not. CONTAINER=echo prints the
//! command the recipe would run instead of running it.

use std::path::Path;
use std::process::{Command, Output};

fn zizmor(token: Option<&str>) -> Output {
    let mut cmd = Command::new(std::env::var("JUST").unwrap_or("just".into()));
    cmd.arg("--justfile")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Justfile"))
        .args(["--working-directory", env!("CARGO_MANIFEST_DIR"), "zizmor"])
        .env("CONTAINER", "echo")
        .env_remove("GH_TOKEN");
    if let Some(token) = token {
        cmd.env("GH_TOKEN", token);
    }
    let out = cmd
        .output()
        .expect("just, as JUST from the test recipe or on the path");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

#[test]
fn online_with_a_token() {
    let out = zizmor(Some("test-token-not-secret"));
    let args = String::from_utf8(out.stdout).unwrap();
    assert!(!args.contains("--offline"), "{args}");
    assert!(args.trim_end().ends_with(" --collect=all ."), "{args}");
    // Forwarded by name, so the value is not on the command line.
    assert!(args.contains(" -e GH_TOKEN "), "{args}");
    assert!(!args.contains("test-token-not-secret"), "{args}");
}

#[test]
fn offline_without_a_token() {
    for token in [None, Some("")] {
        let out = zizmor(token);
        let args = String::from_utf8(out.stdout).unwrap();
        assert!(
            args.trim_end().ends_with(" --offline --collect=all ."),
            "{args}"
        );
        let note = String::from_utf8(out.stderr).unwrap();
        assert!(note.contains("GH_TOKEN is not set"), "{note}");
    }
}
