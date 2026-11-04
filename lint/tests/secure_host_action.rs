//! secure-host/action.yml's own bash steps, each run from the file as the
//! runner runs it, with a fake sudo on PATH in place of podman.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use yaml_rust2::{Yaml, YamlLoader};

const PIN_STEP: &str = "Read the pin";
const CONFIG_STEP: &str = "Write the configuration";
const PROXY_STEP: &str = "Trust the per-job proxy in later JavaScript and Python steps";

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Records its arguments, one to a line, and answers as FAKE_SUDO_* say.
const FAKE_SUDO: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$FAKE_SUDO_ARGS"
printf '%s' "$FAKE_SUDO_STDOUT"
printf '%s' "$FAKE_SUDO_STDERR" >&2
exit "${FAKE_SUDO_STATUS:-0}"
"#;

fn secure_host() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../secure-host")
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "secure-host-action-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.path(name)).ok()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A step's script and the names of its environment.
struct Step {
    run: String,
    env: Vec<String>,
}

fn step(name: &str) -> Step {
    let text = fs::read_to_string(secure_host().join("action.yml")).unwrap();
    let action = YamlLoader::load_from_str(&text).unwrap().remove(0);
    let step = action["runs"]["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .find(|step| step["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("no step {name:?}"));
    assert_eq!(step["shell"].as_str(), Some("bash"), "{name}");
    let env = match &step["env"] {
        Yaml::Hash(env) => env.keys().map(|k| k.as_str().unwrap().to_owned()).collect(),
        _ => Vec::new(),
    };
    Step {
        run: step["run"].as_str().unwrap().to_owned(),
        env,
    }
}

impl Step {
    /// Runs the script as the runner does, in TEMP as RUNNER_TEMP and with
    /// its GITHUB_OUTPUT and GITHUB_ENV, after the fake sudo on PATH. Every
    /// name of the step's environment must be given, as the runner would.
    fn run(&self, temp: &TempDir, env: &[(&str, &Path)], extra: &[(&str, &str)]) -> Output {
        let given: BTreeMap<_, _> = env.iter().copied().collect();
        for name in &self.env {
            assert!(
                given.contains_key(name.as_str()),
                "the test gives no {name}"
            );
        }
        for name in given.keys() {
            assert!(self.env.iter().any(|n| n == name), "the step has no {name}");
        }
        let bin = temp.path("bin");
        fs::create_dir_all(&bin).unwrap();
        let sudo = bin.join("sudo");
        fs::write(&sudo, FAKE_SUDO).unwrap();
        fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                &self.run,
            ])
            .env_clear()
            .env("PATH", path)
            .env("RUNNER_TEMP", &temp.0)
            .env("GITHUB_OUTPUT", temp.path("output"))
            .env("GITHUB_ENV", temp.path("env"))
            .env("FAKE_SUDO_ARGS", temp.path("sudo-args"))
            .envs(env.iter().copied())
            .envs(extra.iter().copied())
            .output()
            .expect("bash")
    }
}

fn failed_with(out: &Output, want: &str) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "succeeded: {text}");
    assert!(text.contains(want), "no {want:?} in {text}");
}

fn succeeded(out: &Output) {
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out + "\""
}

/// signer.json with CHANGES applied, None removing a key.
fn signer(changes: &[(&str, Option<&str>)]) -> String {
    let mut signer: BTreeMap<&str, Option<&str>> = [
        ("repository", Some("example/builder")),
        ("workflow", Some(".github/workflows/release.yml")),
        ("ref", Some("refs/heads/main")),
    ]
    .into();
    signer.extend(changes.iter().copied());
    let fields: Vec<_> = signer
        .iter()
        .filter_map(|(k, v)| v.map(|v| format!("{}: {}", json_string(k), json_string(v))))
        .collect();
    format!("{{{}}}", fields.join(", "))
}

fn pin(pin: &str, signer: &str) -> (TempDir, Output) {
    let temp = TempDir::new();
    temp.write("Containerfile.pin", pin);
    temp.write("signer.json", signer);
    let out = step(PIN_STEP).run(&temp, &[("ACTION", &temp.0)], &[]);
    (temp, out)
}

#[test]
fn pin_names_image_by_digest_and_signer_by_workflow_identity() {
    let (temp, out) = pin(
        &format!("FROM ghcr.io/example/secure-host:v1.2.3@{DIGEST}\n"),
        &signer(&[]),
    );
    succeeded(&out);
    assert_eq!(
        temp.read("output").unwrap(),
        format!(
            "image=ghcr.io/example/secure-host@{DIGEST}\nrepository=example/builder\n\
             cert-identity=https://github.com/example/builder/.github/workflows/release.yml@refs/heads/main\n\
             source-ref=refs/heads/main\n"
        )
    );
    // Without a tag, and with a registry port.
    let (temp, out) = pin(&format!("FROM localhost:5000/a/b@{DIGEST}"), &signer(&[]));
    succeeded(&out);
    let output = temp.read("output").unwrap();
    assert_eq!(
        output.lines().next(),
        Some(format!("image=localhost:5000/a/b@{DIGEST}").as_str())
    );
}

#[test]
fn pin_without_digest_or_with_more_than_one_line_is_refused() {
    for text in [
        "FROM ghcr.io/example/secure-host:v1.2.3".to_owned(),
        format!("FROM ghcr.io/example/secure-host@sha256:{}", "0".repeat(63)),
        format!("FROM ghcr.io/example/secure-host@{DIGEST}\nFROM ghcr.io/other/image@{DIGEST}"),
        format!("FROM ghcr.io/example/secure-host@{DIGEST} --privileged"),
        format!("FROM --platform=linux/amd64 ghcr.io/example/secure-host@{DIGEST}"),
        format!("from ghcr.io/example/secure-host@{DIGEST}"),
        String::new(),
    ] {
        let (temp, out) = pin(&text, &signer(&[]));
        failed_with(&out, "Containerfile.pin");
        assert_eq!(temp.read("output"), None, "{text:?}");
    }
}

#[test]
fn missing_or_malformed_signer_is_refused_newlines_included() {
    for change in [
        ("repository", None),
        ("repository", Some("example")),
        ("repository", Some("example/builder\ninjected=value")),
        ("workflow", Some("release.yml")),
        ("workflow", Some(".github/workflows/../release.yml\n")),
        ("ref", Some("main")),
        ("ref", Some("refs/heads/main\nx=y")),
    ] {
        let (temp, out) = pin(
            &format!("FROM ghcr.io/example/secure-host@{DIGEST}"),
            &signer(&[change]),
        );
        failed_with(&out, "signer.json");
        assert_eq!(temp.read("output"), None, "{change:?}");
    }
}

#[test]
fn checked_in_pin_and_signer_are_valid_and_written_as_outputs() {
    let temp = TempDir::new();
    let out = step(PIN_STEP).run(&temp, &[("ACTION", &secure_host())], &[]);
    succeeded(&out);
    let output = temp.read("output").unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 4, "{output}");
    let (name, digest) = lines[0]
        .strip_prefix("image=")
        .and_then(|image| image.split_once("@sha256:"))
        .expect(lines[0]);
    assert!(name.contains('/') && !name.contains(':'), "{name}");
    assert!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
}

fn runner_user() -> String {
    let out = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

/// The configuration step's environment: an absent input is empty, as the
/// runner passes it.
fn configure(
    temp: &TempDir,
    egress: &str,
    policy: &Path,
    config: &Path,
    sudo: &[(&str, &str)],
) -> Output {
    let action = secure_host();
    let env = [
        ("ACTION", action.as_path()),
        ("CONFIG", config),
        ("EGRESS", Path::new(egress)),
        ("EGRESS_POLICY", policy),
        ("ID", Path::new("image")),
    ];
    step(CONFIG_STEP).run(temp, &env, sudo)
}

/// The fake sudo's record of the configuration command.
fn podman(temp: &TempDir, settings: &[&str]) -> String {
    let volume = format!("{}/secure-host/in:/in:ro", temp.0.display());
    let user = format!("sandbox.runner-user={}", runner_user());
    let mut args = vec![
        "podman",
        "run",
        "--rm",
        "--network=none",
        "--entrypoint",
        "/agentic-job",
        "-v",
        &volume,
        "image",
        "config",
        "--host",
        "--from",
        "/in/config.toml",
        "--string",
        &user,
    ];
    args.extend(settings);
    args.join("\n") + "\n"
}

#[test]
fn egress_inputs_compose_as_arguments_and_copy_policy_to_a_host_path() {
    for (egress, with_policy) in [
        ("off", false),
        ("", false),
        ("write-policy", false),
        ("write-policy", true),
    ] {
        let temp = TempDir::new();
        let policy_text = "[[write]]\nhost = \"example.com\"\n";
        let policy = temp.write("policy with spaces.toml", policy_text);
        let host_policy = temp.path("secure-host/policy.toml");
        let host_policy_arg = format!("egress.policy={}", host_policy.display());
        let mut settings = vec!["--boolean"];
        if egress == "write-policy" {
            settings.extend([
                "egress.write-policy=true",
                "--boolean",
                "egress.proxy=true",
                "--boolean",
                "egress.rust-proxy=true",
            ]);
            if with_policy {
                settings.extend(["--string", &host_policy_arg]);
            }
        } else {
            settings.push("egress.write-policy=false");
        }
        let given = if with_policy {
            policy.as_path()
        } else {
            Path::new("")
        };
        let out = configure(
            &temp,
            egress,
            given,
            Path::new(""),
            &[("FAKE_SUDO_STDOUT", "checked config\n\n")],
        );
        succeeded(&out);
        assert_eq!(
            temp.read("sudo-args").unwrap(),
            podman(&temp, &settings),
            "{egress} {with_policy}"
        );
        // stdout as it was, trailing newlines included.
        assert_eq!(
            temp.read("secure-host/config.toml").unwrap(),
            "checked config\n\n"
        );
        assert_eq!(
            temp.read("secure-host/in/config.toml").unwrap(),
            fs::read_to_string(secure_host().join("hosted.toml")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(&host_policy).ok(),
            with_policy.then(|| policy_text.to_owned())
        );
    }
}

#[test]
fn a_config_input_is_copied_in_place_of_hosted_toml() {
    let temp = TempDir::new();
    let config = temp.write("mine.toml", "[sandbox]\n");
    succeeded(&configure(&temp, "off", Path::new(""), &config, &[]));
    assert_eq!(
        temp.read("secure-host/in/config.toml").unwrap(),
        "[sandbox]\n"
    );
}

#[test]
fn bad_egress_inputs_and_unreadable_files_fail_before_the_image_runs() {
    for (egress, policy, want) in [
        ("yes", None, "egress must be off or write-policy"),
        (
            "off",
            Some("policy.toml"),
            "egress-policy needs egress: write-policy",
        ),
        // Its rules are parsed only by the image's proxy at setup, which
        // the action waits for: a file that cannot be read fails here.
        ("write-policy", Some("missing.toml"), "missing.toml"),
        ("write-policy", Some("directory"), "directory"),
    ] {
        let temp = TempDir::new();
        temp.write("policy.toml", "");
        fs::create_dir(temp.path("directory")).unwrap();
        let policy = policy.map(|p| temp.path(p)).unwrap_or_default();
        failed_with(&configure(&temp, egress, &policy, Path::new(""), &[]), want);
        assert_eq!(temp.read("sudo-args"), None, "{egress} {policy:?}");
    }
}

#[test]
fn a_refused_configuration_fails_with_its_stderr_and_writes_none() {
    let temp = TempDir::new();
    let sudo = [
        ("FAKE_SUDO_STATUS", "1"),
        ("FAKE_SUDO_STDOUT", "partial"),
        ("FAKE_SUDO_STDERR", "lock-runner required"),
    ];
    let out = configure(&temp, "off", Path::new(""), Path::new(""), &sudo);
    failed_with(&out, "host configuration refused: lock-runner required");
    assert_eq!(temp.read("secure-host/config.toml"), None);
}

// The CI fixture replaces the image's policy, and the action's check
// (`egress-git`) fetches from github.com: a fixture without the git fetch
// rule fails that check on the hosted runner, not here.
#[test]
fn ci_egress_policy_fixture_keeps_git_fetch_from_github() {
    let text = fs::read_to_string(secure_host().join("ci-egress-policy.toml")).unwrap();
    let mut rules: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if line == "[[write]]" {
            rules.push(Vec::new());
        } else if let Some(rule) = rules.last_mut() {
            rule.push(line);
        }
    }
    let github: Vec<_> = rules
        .iter()
        .filter(|rule| {
            rule.iter().any(|line| {
                line.strip_prefix("hosts = [")
                    .and_then(|hosts| hosts.split(']').next())
                    .is_some_and(|hosts| hosts.contains("\"github.com\""))
            })
        })
        .collect();
    // Only the fetch: a wider github.com rule would let a push (git-receive-pack) through.
    assert_eq!(github.len(), 1, "{github:?}");
    for want in [
        r#"hosts = ["github.com"]"#,
        r#"methods = ["POST"]"#,
        r#"paths = ["/**/git-upload-pack"]"#,
    ] {
        assert!(github[0].contains(&want), "no {want} in {:?}", github[0]);
    }
}

#[test]
fn runner_env_passes_only_trust_store_paths_under_the_proxy_directory() {
    let file = "NODE_EXTRA_CA_CERTS=/etc/egress-proxy/ca.pem\nREQUESTS_CA_BUNDLE=/etc/egress-proxy/ca-bundle.pem\n";
    let cases = [
        (format!("# exported\n{file}\n"), Ok(file)),
        // The last line without its newline.
        (file.trim_end().to_owned(), Ok(file)),
        (String::new(), Err("runner.env: no trust store to export")),
        (
            "# only a comment\n\n".to_owned(),
            Err("runner.env: no trust store to export"),
        ),
        (
            "NODE_OPTIONS=--require=/tmp/x.js".to_owned(),
            Err("runner.env: refused"),
        ),
        (
            "BASH_ENV=/etc/egress-proxy/ca.pem".to_owned(),
            Err("runner.env: refused"),
        ),
        (
            "NODE_EXTRA_CA_CERTS=/tmp/ca.pem".to_owned(),
            Err("runner.env: refused"),
        ),
        (
            "NODE_EXTRA_CA_CERTS=/etc/egress-proxy/..".to_owned(),
            Err("runner.env: refused"),
        ),
        (
            "NODE_EXTRA_CA_CERTS=/etc/egress-proxy/ca.pem\nX<<EOF".to_owned(),
            Err("runner.env: refused"),
        ),
    ];
    for (text, want) in cases {
        let temp = TempDir::new();
        let runner_env = temp.write("runner.env", &text);
        let out = step(PROXY_STEP).run(
            &temp,
            &[("ACTION", &secure_host()), ("PROXY_ENV", &runner_env)],
            &[],
        );
        match want {
            Ok(want) => {
                succeeded(&out);
                assert_eq!(temp.read("env").as_deref(), Some(want), "{text:?}");
            }
            Err(want) => {
                failed_with(&out, want);
                assert_eq!(temp.read("env"), None, "{text:?}");
            }
        }
    }
}
