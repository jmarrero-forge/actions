use std::{collections::BTreeSet, path::Path, process::Command};

// Existing scripts remain allowed until their pinned Rust replacement ships.
// New Node entry points would bypass the migration and its delivery checks.
const ALLOWED: &[&str] = &[
    ".github/workflows/cgwalters-agentic-job.test.mjs",
    "app-token/remint.mjs",
    "app-token/remint.test.mjs",
    "renovate/dry-run.mjs",
    "sandbox-share/check.mjs",
    "sandbox-share/share.cjs",
    "secure-host/egress-probe.mjs",
    "secure-host/egress-probe.test.mjs",
    "secure-host/read-probe.mjs",
    "secure-host/read-probe.test.mjs",
    "tailscale/check.mjs",
    "tailscale/check.test.mjs",
    "tailscale/hardening.test.mjs",
    "tailscale/mint.mjs",
    "tailscale/mint.test.mjs",
    "tailscale/prerequisites.test.mjs",
    "tailscale/setup.mjs",
    "tailscale/setup.test.mjs",
    // Grandfather the existing prose advisory alongside the action scripts
    // during migration; unrelated new Node entry points remain denied.
    "workflow/prose-binary.test.mjs",
    "workflow/prose.mjs",
    "workflow/prose.test.mjs",
];

#[test]
fn no_new_javascript_outside_allowlist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*.mjs",
            "*.cjs",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "git ls-files failed");
    let paths = String::from_utf8(output.stdout).unwrap();
    let unexpected: BTreeSet<_> = paths
        .split('\0')
        .filter(|p| !p.is_empty() && !ALLOWED.contains(p))
        .collect();
    assert!(
        unexpected.is_empty(),
        "new JavaScript outside migration allowlist: {unexpected:?}"
    );
}
