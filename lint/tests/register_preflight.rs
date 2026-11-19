//! Execute the actual register preflight from each wrapper, without credentials.

use std::{fs, path::Path, process::Command};

use yaml_rust2::YamlLoader;

const PREFLIGHT: &str = "Refuse unsupported child agents before credentials";

#[test]
fn register_preflight_precedes_credentials_and_preserves_configuration() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut scripts = Vec::new();
    for file in ["cgwalters-agentic-job.yml", "cgwalters-review.yml"] {
        let text = fs::read_to_string(root.join(".github/workflows").join(file)).unwrap();
        let documents = YamlLoader::load_from_str(&text).unwrap();
        let steps = documents[0]["jobs"]["register"]["steps"]
            .as_vec()
            .expect("register steps");
        let named = |name: &str| {
            steps
                .iter()
                .position(|step| step["name"].as_str() == Some(name))
                .unwrap()
        };
        let preflight = named(PREFLIGHT);
        let provenance = named("The policy call's source and uploads");
        let download = steps
            .iter()
            .position(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("actions/download-artifact@"))
            })
            .unwrap();
        assert!(provenance < download && download < preflight, "{file}");
        assert!(
            steps
                .iter()
                .position(|step| {
                    step["name"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("Join the tailnet and mint"))
                })
                .unwrap()
                > preflight,
            "{file}"
        );
        assert!(
            steps
                .iter()
                .position(|step| step["id"].as_str() == Some("register"))
                .unwrap()
                > preflight,
            "{file}"
        );

        // Keep the original textual guard too: secrets must not enter an earlier
        // step through env, with, or an expression outside the run script.
        let register = text.split("\n  register:\n").nth(1).unwrap();
        let before = register
            .split(&format!("- name: {PREFLIGHT}"))
            .next()
            .unwrap();
        for step in &steps[..preflight] {
            if let Some(action) = step["uses"].as_str() {
                assert!(
                    !action.contains("tailscale") && !action.contains("checkout"),
                    "{file}: premature action {action}"
                );
            }
        }
        for forbidden in ["agentic-job\" register", "secrets."] {
            assert!(!before.contains(forbidden), "{file}: premature {forbidden}");
        }
        let source = steps[provenance]["run"].as_str().unwrap();
        assert!(source.contains(r#"[[ "$SOURCE_REPOSITORY" == "$EXPECTED_REPOSITORY" && "$SOURCE_SHA" =~ ^[0-9a-f]{40}$"#), "{file}");
        assert!(
            source.contains(r#""$BINARY" =~ ^[1-9][0-9]*$ && "$POLICY" =~ ^[1-9][0-9]*$"#),
            "{file}"
        );
        assert_eq!(
            steps[download]["with"]["artifact-ids"].as_str(),
            Some(
                "${{ needs.policy.outputs.binary-artifact-id }},${{ needs.policy.outputs.policy-artifact-id }}"
            ),
            "{file}"
        );
        let step = &steps[preflight];
        assert_eq!(step["shell"].as_str(), Some("python3 {0}"), "{file}");
        assert_eq!(
            step["env"]["JOB_DIR"].as_str(),
            Some("${{ runner.temp }}/agentic-job"),
            "{file}"
        );
        let script = step["run"].as_str().unwrap();
        scripts.push(script.to_owned());

        let mut at_limit = b"[agent]\nname = 'opencode'\n#".to_vec();
        at_limit.resize(1_048_576, b'#');
        let mut oversized = at_limit.clone();
        oversized.push(b'#');
        let cases: &[(&str, Option<&[u8]>, i32)] = &[
            ("ordinary", Some(b"[agent]\nname = \"opencode\"\n[session]\nwatch-item = true"), 0),
            ("comment/string is not a child", Some(b"# [child-agent]\n[agent]\nname = \"claude\"\n[host.env]\nNOTE = \"[child-agent]\""), 0),
            ("child table", Some(b"[agent]\nname = \"vibe\"\n[child-agent]\nname = \"vibe\""), 1),
            ("empty child", Some(b"[child-agent]"), 1),
            ("quoted child", Some(b"[\"child-agent\"]\nname = \"vibe\""), 1),
            ("dotted child", Some(b"child-agent.name = \"vibe\""), 1),
            ("inline child", Some(b"child-agent = {name = \"vibe\"}"), 1),
            ("false child", Some(b"child-agent = false"), 1),
            ("malformed", Some(b"[agent"), 1),
            ("duplicate", Some(b"[agent]\nname = \"vibe\"\nname = \"claude\""), 1),
            ("invalid utf8", Some(&[0xff]), 1),
            ("empty", Some(b""), 1),
            ("at size limit", Some(&at_limit), 0),
            ("oversized", Some(&oversized), 1),
            ("missing", None, 1),
        ];
        for &(label, config, status) in cases {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            if let Some(config) = config {
                fs::write(&path, config).unwrap();
            }
            let output = Command::new("python3")
                .args(["-c", script])
                .env("JOB_DIR", dir.path())
                .output()
                .expect("run register preflight with Python 3.11+ (tomllib)");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(status),
                "{file}: {label}: {stderr}"
            );
            if status != 0 {
                assert!(
                    stderr.contains("register preflight:"),
                    "{file}: {label}: {stderr}"
                );
                if label.contains("child") {
                    assert!(stderr.contains("acp-experiment is unsupported under the pinned runner-only implementation; remove [child-agent]"), "{file}: {label}: {stderr}");
                }
            }
            if let Some(config) = config {
                assert_eq!(
                    fs::read(&path).unwrap(),
                    config,
                    "{file}: {label}: preflight must not rewrite configuration"
                );
            } else {
                assert!(
                    !path.exists(),
                    "{file}: preflight must not create configuration"
                );
            }
        }
    }
    assert_eq!(
        scripts[0], scripts[1],
        "both wrappers must enforce the same preflight"
    );
}
