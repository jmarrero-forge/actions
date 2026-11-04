//! Run the small composite actions' actual Bash steps, with isolated inputs
//! and GITHUB_OUTPUT, rather than reimplementing their validation in tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use yaml_rust2::YamlLoader;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "composite-actions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(action: &str, text: &str, owner_id: Option<&str>) -> (Output, String) {
    let temp = TempDir::new();
    let source = fs::read_to_string(root().join(format!("{action}/action.yml"))).unwrap();
    let yaml = YamlLoader::load_from_str(&source).unwrap().remove(0);
    let steps = yaml["runs"]["steps"].as_vec().unwrap();
    assert_eq!(steps.len(), 1);
    let step = &steps[0];
    assert_eq!(step["shell"].as_str(), Some("bash"));
    assert_eq!(
        step["env"]["ACTION_PATH"].as_str(),
        Some("${{ github.action_path }}")
    );
    if action == "admit" {
        assert_eq!(
            step["env"]["OWNER"].as_str(),
            Some("${{ github.repository_owner }}")
        );
        assert_eq!(
            step["env"]["OWNER_ID"].as_str(),
            Some("${{ github.repository_owner_id }}")
        );
    }
    fs::write(
        temp.0.join(if action == "admit" {
            "owners.json"
        } else {
            "deployment.json"
        }),
        text,
    )
    .unwrap();
    let output_path = temp.0.join("output");
    fs::write(&output_path, "").unwrap();
    let mut command = Command::new("bash");
    command
        .args([
            "--noprofile",
            "--norc",
            "-e",
            "-o",
            "pipefail",
            "-c",
            step["run"].as_str().unwrap(),
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("ACTION_PATH", &temp.0)
        .env("GITHUB_OUTPUT", &output_path)
        .env("OWNER", "caller");
    if let Some(id) = owner_id {
        command.env("OWNER_ID", id);
    }
    let output = command.output().expect("bash and jq must be installed");
    (output, fs::read_to_string(output_path).unwrap())
}

fn diagnostics(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn owners_are_positive_safe_integer_ids_and_only_exact_listed_ids_are_admitted() {
    let text = fs::read_to_string(root().join("admit/owners.json")).unwrap();
    let owners = YamlLoader::load_from_str(&text).unwrap().remove(0);
    let owners = owners.as_hash().unwrap();
    assert!(!owners.is_empty());
    for (login, id) in owners {
        let id = id.as_i64().unwrap();
        assert!(id > 0 && id <= 9_007_199_254_740_991, "{login:?}");
        let (out, written) = run("admit", &text, Some(&id.to_string()));
        assert!(out.status.success(), "{}", diagnostics(&out));
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("caller (id {id}) is admitted\n")
        );
        assert!(written.is_empty());
    }
    let (login, id) = owners.iter().next().unwrap();
    let listed = id.as_i64().unwrap().to_string();
    let cases = [
        None,
        Some(String::new()),
        Some("0".into()),
        Some("1".into()),
        Some(login.as_str().unwrap().into()),
        Some(format!(" {listed}")),
        Some(format!("{listed} ")),
        Some(format!("0{listed}")),
        Some(format!("{listed}.0")),
        Some(format!("[{listed}]")),
        Some(format!("{listed}\n")),
    ];
    for id in cases {
        let (out, written) = run("admit", &text, id.as_deref());
        assert_eq!(out.status.code(), Some(1), "{id:?}: {}", diagnostics(&out));
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!(
                "::error::caller (id {}) is not an owner admit/owners.json lists\n",
                id.as_deref().unwrap_or("")
            )
        );
        assert!(written.is_empty());
    }
}

#[test]
fn deployment_defaults_and_outputs() {
    let ours = fs::read_to_string(root().join("deployment/deployment.json")).unwrap();
    for (text, expected) in [
        (
            ours.as_str(),
            "agent-config-repo=jmarrero-forge/agenthome\nagent-model=claude-opus-5-5\n",
        ),
        (
            r#"{"agent-config-repo":"example/home","agent-model":"devstral-small-latest"}"#,
            "agent-config-repo=example/home\nagent-model=devstral-small-latest\n",
        ),
    ] {
        let (out, written) = run("deployment", text, None);
        assert!(out.status.success(), "{}", diagnostics(&out));
        assert_eq!(written, expected);
    }
}

#[test]
fn missing_malformed_or_unknown_deployment_settings_fail_without_outputs() {
    let cases = [
        ("", "deployment.json must be an object"),
        ("[]", "deployment.json must be an object"),
        ("null", "deployment.json must be an object"),
        ("{}", "missing setting agent-config-repo"),
        ("{} {}", "deployment.json must be an object"),
        (
            r#"{"agent-config-repo":"example/home"}"#,
            "missing setting agent-model",
        ),
        (
            r#"{"agent-model":"p/m"}"#,
            "missing setting agent-config-repo",
        ),
        (
            r#"{"agent-config-repo":"example/home","agent-model":"devstral-small-latest","agent-config-rep":"o/d"}"#,
            "unknown setting agent-config-rep",
        ),
    ];
    for (text, error) in cases {
        let (out, written) = run("deployment", text, None);
        assert!(!out.status.success(), "{text}");
        assert!(diagnostics(&out).contains(error), "{}", diagnostics(&out));
        assert!(written.is_empty(), "{text}: {written}");
    }
    for (key, values) in [
        (
            "agent-config-repo",
            vec![
                r#""""#,
                r#""o/c\nx=y""#,
                r#""no-slash""#,
                r#""o/c\n""#,
                "42",
                "null",
                "false",
            ],
        ),
        (
            "agent-model",
            vec![
                r#""""#,
                r#""p/m\nx=y""#,
                r#""a/b/c""#,
                "42",
                r#""p/m\n""#,
                "null",
                "false",
            ],
        ),
    ] {
        for value in values {
            let repo = if key == "agent-config-repo" {
                value
            } else {
                r#""example/home""#
            };
            let model = if key == "agent-model" {
                value
            } else {
                r#""devstral-small-latest""#
            };
            let text = format!(r#"{{"agent-config-repo":{repo},"agent-model":{model}}}"#);
            let (out, written) = run("deployment", &text, None);
            assert!(!out.status.success(), "{text}");
            assert!(
                diagnostics(&out).contains(&format!("{key} must match")),
                "{text}: {}",
                diagnostics(&out)
            );
            assert!(written.is_empty(), "{text}: {written}");
        }
    }
}
