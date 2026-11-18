//! The zizmor recipe runs the online audits when GH_TOKEN is set and falls
//! back to --offline, with a note, when it is not. CONTAINER=echo prints the
//! command the recipe would run instead of running it.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

// Opt-in because normal unit tests need neither a GitHub token nor an installed
// zizmor. Run with the version from Containerfile.pin; no repository config or
// ignore can hide a finding in these fixtures.
#[test]
#[ignore = "requires GH_TOKEN and zizmor on PATH (or ZIZMOR)"]
fn online_self_pins_survive_main_advancing() {
    assert!(!std::env::var("GH_TOKEN").unwrap_or_default().is_empty());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut pins = Vec::new();
    for file in [
        "cgwalters-agentic-job",
        "cgwalters-review",
        "cgwalters-agent",
    ] {
        let text =
            std::fs::read_to_string(root.join(format!(".github/workflows/{file}.yml"))).unwrap();
        for line in text.lines() {
            if let Some((_, pin)) = line.split_once("uses: cgwalters-forge/actions/")
                && pin.contains(" # from main")
            {
                pins.push(format!("      - uses: cgwalters-forge/actions/{pin}\n"));
            }
        }
    }
    // Include the independent advisory register join and the pinned host action.
    assert_eq!(pins.len(), 9, "exercise every immutable wrapper self-pin");
    let checkout = "      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with: {persist-credentials: false}\n";
    let audit = |steps: &str| {
        let mut child = Command::new(std::env::var("ZIZMOR").unwrap_or("zizmor".into()))
            .args(["--no-config", "--no-progress", "--color=never", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("installed zizmor");
        write!(child.stdin.take().unwrap(),
            "name: self-pin regression\non: push\npermissions: {{}}\njobs:\n  test:\n    runs-on: ubuntu-26.04\n    timeout-minutes: 5\n    steps:\n{steps}"
        ).unwrap();
        let out = child.wait_with_output().unwrap();
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status, diagnostics)
    };
    let steps = pins.concat() + checkout;
    let (status, diagnostics) = audit(&steps);
    assert!(status.success(), "{diagnostics}");
    // Use a fixed historical pin, not today's wrapper pins: a Renovate PR
    // may legitimately update every wrapper pin to main's current tip.
    // This is the failing pin from run 42488914465, now behind main.
    let historical = "      - uses: cgwalters-forge/actions/admit@e084c52a9aa07f1279182cfc738de68b96016278 # from main\n";
    let (status, diagnostics) = audit(historical);
    assert!(status.success(), "{diagnostics}");
    let (status, diagnostics) = audit(&historical.replace("# from main", "# main"));
    assert_eq!(status.code(), Some(13), "{diagnostics}");
    assert!(
        diagnostics.contains("warning[ref-version-mismatch]"),
        "{diagnostics}"
    );
    // The prose convention must not disable third-party ref/version checks.
    let (status, diagnostics) = audit(&steps.replace("# v7.0.1", "# v1"));
    assert_eq!(status.code(), Some(13), "{diagnostics}");
    assert!(
        diagnostics.contains("warning[ref-version-mismatch]"),
        "{diagnostics}"
    );
}
