//! workflows-check over a pull request in a real Git repository, with the
//! fixture workflows in tests/workflows-check/ and a stand-in for the shape
//! check that shows what it was given.

use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/workflows-check");

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("workflows-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".github/workflows")).unwrap();
        // Prints the files it checks and the exemptions it was given, and
        // exits with SHAPE_EXIT.
        std::fs::write(
            dir.join("shape"),
            "#!/bin/sh\nls \"$4/.github/workflows\"\necho \"exemptions: $(cat \"$6\")\"\nexit \"${SHAPE_EXIT:-0}\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir.join("shape"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let repo = Repo(dir.join("repo"));
        std::fs::create_dir_all(repo.0.join(".github/workflows")).unwrap();
        repo.git(&["init", "-q"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Writes each file from a fixture, or from the text given when there is
    /// no such fixture, or removes it with None; then commits.
    fn commit(&self, files: &[(&str, Option<&str>)]) -> String {
        for (path, fixture) in files {
            let path = self.0.join(path);
            match fixture {
                Some(fixture) => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let source = Path::new(FIXTURES).join(fixture);
                    let text =
                        std::fs::read_to_string(&source).unwrap_or_else(|_| fixture.to_string());
                    std::fs::write(path, text).unwrap();
                }
                None => std::fs::remove_file(path).unwrap(),
            }
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "c"]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn check(&self, base: &str, head: &str, shape_exit: &str) -> (Option<i32>, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_workflows-check"))
            .arg("--shape-check")
            .arg(self.0.parent().unwrap().join("shape"))
            .arg(&self.0)
            .args([base, head])
            .env("SHAPE_EXIT", shape_exit)
            .env_remove("GITHUB_ACTIONS")
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

const CI: &str = ".github/workflows/ci.yml";
const CALLER: &str = ".github/workflows/workflows.yml";

fn pull_request(name: &str, files: &[(&str, Option<&str>)]) -> (Option<i32>, String) {
    let repo = Repo::new(name);
    let base = repo.commit(&[(CI, Some("base.yml")), (CALLER, Some("caller.yml"))]);
    let head = repo.commit(files);
    repo.check(&base, &head, "0")
}

#[test]
fn passing() {
    let (code, out) = pull_request("pass", &[(CI, Some("pass.yml"))]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("ci.yml\nworkflows.yml\n"), "{out}");
}

/// secure-host's plain-CI example, as a repository's pull request that adds
/// it with no exemption: the rules here admit it, and it reaches the shape
/// check unexempted. The stand-in judges nothing of its jobs; the shape
/// check lint/shape-check.yml names does.
#[test]
fn plain_ci_example() {
    let example = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.github/workflows/example-plain-ci.yml"
    ))
    .unwrap();
    let (code, out) = pull_request(
        "plain-ci",
        &[(".github/workflows/example-plain-ci.yml", Some(&example))],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("example-plain-ci.yml\n"), "{out}");
    assert!(out.contains("exemptions: \n"), "{out}");
    let exemptions = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.github/agentic-job/workflows.toml"
    ))
    .unwrap();
    assert!(!exemptions.contains("example-plain-ci"));
}

fn plain_ci() -> yaml_rust2::Yaml {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../.github/workflows/example-plain-ci.yml"
    ))
    .unwrap();
    yaml_rust2::YamlLoader::load_from_str(&text)
        .unwrap()
        .remove(0)
}

#[test]
fn plain_ci_produces_one_file_and_exports_its_digest() {
    let workflow = plain_ci();
    let build = &workflow["jobs"]["build"];
    let steps = build["steps"].as_vec().unwrap();
    let hardening = steps
        .iter()
        .find(|s| s["with"]["egress"].as_str().is_some())
        .unwrap();
    assert!(
        hardening["uses"]
            .as_str()
            .unwrap()
            .starts_with("cgwalters-forge/actions/secure-host@")
    );
    assert_eq!(hardening["with"].as_hash().unwrap().len(), 1);
    assert_eq!(hardening["with"]["egress"].as_str(), Some("write-policy"));
    let payload = steps
        .iter()
        .find(|s| s["id"].as_str() == Some("payload"))
        .unwrap();
    assert_eq!(
        build["outputs"]["digest"].as_str(),
        Some("${{ steps.payload.outputs.digest }}")
    );
    assert_eq!(
        build["outputs"]["id"].as_str(),
        Some("${{ steps.up.outputs.artifact-id }}")
    );
    let upload = steps
        .iter()
        .find(|s| s["id"].as_str() == Some("up"))
        .unwrap();
    assert_eq!(upload["with"]["path"].as_str(), Some("artifact"));
    let repo = Repo::new("plain-payload");
    let commit = repo.commit(&[]);
    let output = repo.0.parent().unwrap().join("output");
    let out = Command::new("bash")
        .args([
            "--noprofile",
            "--norc",
            "-e",
            "-o",
            "pipefail",
            "-c",
            payload["run"].as_str().unwrap(),
        ])
        .current_dir(&repo.0)
        .env("GITHUB_OUTPUT", &output)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(repo.0.join("artifact")).unwrap(),
        format!("{commit}\n")
    );
    let hash = Command::new("sha256sum")
        .arg(repo.0.join("artifact"))
        .output()
        .unwrap();
    let hash = String::from_utf8(hash.stdout).unwrap();
    assert_eq!(
        std::fs::read_to_string(output).unwrap(),
        format!("digest={}\n", hash.split_whitespace().next().unwrap())
    );
}

/// Run the consumer scripts straight from the workflow, without a checkout
/// or producer code. Only these data-only scripts run; never host setup.
#[test]
fn plain_ci_consumer_rejects_bad_layout_digests_and_commit_data() {
    let workflow = plain_ci();
    let publish = &workflow["jobs"]["publish"];
    assert!(publish["environment"].is_badvalue());
    assert!(publish["env"].is_badvalue());
    let steps = publish["steps"].as_vec().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[0]["uses"].as_str(),
        Some("actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333")
    );
    assert_eq!(steps[0]["with"].as_hash().unwrap().len(), 1);
    assert_eq!(steps[1]["shell"].as_str(), Some("bash"));
    assert_eq!(steps[2]["shell"].as_str(), Some("node {0}"));
    let valid = format!("{}\n", "0123456789abcdef".repeat(2) + "01234567").into_bytes();
    for case in [
        "valid",
        "missing",
        "extra",
        "hidden",
        "directory",
        "symlink",
        "malformed-digest",
        "digest-injection",
        "wrong-digest",
        "short",
        "uppercase",
        "no-newline",
        "two-lines",
        "code",
        "high-bit",
        "oversized",
    ] {
        let repo = Repo::new(&format!("plain-consumer-{case}"));
        let dir = repo.0.parent().unwrap().join("workspace");
        std::fs::create_dir(&dir).unwrap();
        let mut data = valid.clone();
        match case {
            "short" => data = b"abc\n".to_vec(),
            "uppercase" => data = valid.to_ascii_uppercase(),
            "no-newline" => {
                data.pop();
            }
            "two-lines" => data.extend_from_slice(&valid),
            "code" => data = b"require('node:fs').writeFileSync('executed', '')\n".to_vec(),
            "high-bit" => data[0] |= 0x80,
            "oversized" => data = vec![b'a'; 1024 * 1024],
            _ => {}
        }
        std::fs::write(dir.join("artifact"), &data).unwrap();
        let hash = Command::new("sha256sum")
            .arg(dir.join("artifact"))
            .output()
            .unwrap();
        assert!(hash.status.success());
        let hash = String::from_utf8(hash.stdout).unwrap();
        let mut digest = hash.split_whitespace().next().unwrap().to_owned();
        match case {
            "missing" => std::fs::remove_file(dir.join("artifact")).unwrap(),
            "extra" | "hidden" => std::fs::write(
                dir.join(if case == "hidden" { ".extra" } else { "extra" }),
                "unchecked",
            )
            .unwrap(),
            "directory" => {
                std::fs::remove_file(dir.join("artifact")).unwrap();
                std::fs::create_dir(dir.join("artifact")).unwrap();
            }
            "symlink" => {
                std::fs::rename(dir.join("artifact"), repo.0.join("target")).unwrap();
                std::os::unix::fs::symlink(repo.0.join("target"), dir.join("artifact")).unwrap();
            }
            "malformed-digest" => digest = "a".repeat(63),
            "digest-injection" => digest += "\nmalicious checksum entry",
            "wrong-digest" => digest = "0".repeat(64),
            _ => {}
        }
        let check = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                steps[1]["run"].as_str().unwrap(),
            ])
            .current_dir(&dir)
            .env("ARTIFACT_SHA256", digest)
            .output()
            .unwrap();
        let layout_ok = matches!(
            case,
            "valid"
                | "short"
                | "uppercase"
                | "no-newline"
                | "two-lines"
                | "code"
                | "high-bit"
                | "oversized"
        );
        assert_eq!(check.status.success(), layout_ok, "{case}: {check:?}");
        if layout_ok {
            let validate = Command::new("node")
                .args(["-e", steps[2]["run"].as_str().unwrap()])
                .current_dir(&dir)
                .output()
                .unwrap();
            assert_eq!(
                validate.status.success(),
                case == "valid",
                "{case}: {validate:?}"
            );
        }
        assert!(!dir.join("executed").exists());
    }
}

#[test]
fn forbidden_trigger() {
    let (code, out) = pull_request("trigger", &[(CI, Some("trigger.yml"))]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains("ci.yml: adds the trigger `pull_request_target`"),
        "{out}"
    );
    // A byte order mark, which GitHub drops, does not hide it, first or on
    // a later line.
    for (name, bom) in [
        ("bom-first", "\u{feff}on: pull_request_target\njobs: {}\n"),
        (
            "bom-later",
            "# c\n\u{feff}on: pull_request_target\njobs: {}\n",
        ),
    ] {
        let (code, out) = pull_request(name, &[(CI, Some(bom))]);
        assert_eq!(code, Some(1), "{out}");
        assert!(out.contains("ci.yml: contains a byte order mark"), "{out}");
    }
    // A trigger already on the base branch is not the pull request's.
    let repo = Repo::new("trigger-kept");
    let base = repo.commit(&[(CI, Some("trigger.yml"))]);
    let head = repo.commit(&[(".github/workflows/other.yml", Some("base.yml"))]);
    assert_eq!(repo.check(&base, &head, "0").0, Some(0));
}

#[test]
fn new_ignore() {
    let (code, out) = pull_request("ignore", &[(CI, Some("ignore.yml"))]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("ci.yml: line 7 adds a zizmor ignore"), "{out}");
    let (code, out) = pull_request(
        "ignore-action",
        &[("sub/action.yml", Some("# zizmor: ignore[x]\n"))],
    );
    assert_eq!(code, Some(1), "{out}");
    // An added line that itself starts with `++` is still an added line.
    let (code, out) = pull_request(
        "ignore-dependabot",
        &[(
            ".github/dependabot.yml",
            Some("++: x # zizmor: ignore[x]\n"),
        )],
    );
    assert_eq!(code, Some(1), "{out}");
    let (code, out) = pull_request(
        "zizmor-config",
        &[(".github/zizmor.yml", Some("rules: {}\n"))],
    );
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("changes zizmor's configuration"), "{out}");
}

#[test]
fn linter_configuration() {
    for path in [".github/actionlint.yaml", ".github/actionlint.yml"] {
        let config = "paths:\n  .github/workflows/**/*.yml:\n    ignore: ['.*']\n";
        let (code, out) = pull_request("actionlint-config", &[(path, Some(config))]);
        assert_eq!(code, Some(1), "{out}");
        assert!(out.contains("changes actionlint's configuration"), "{out}");
    }
    // A workflow added with the .gitignore line that hides it from zizmor,
    // in the root or deeper.
    for (name, gitignore, pattern) in [
        ("gitignore", ".gitignore", "/.github/workflows/ci.yml\n"),
        (
            "gitignore-nested",
            ".github/.gitignore",
            "workflows/ci.yml\n",
        ),
    ] {
        let (code, out) = pull_request(name, &[(CI, Some("pass.yml")), (gitignore, Some(pattern))]);
        assert_eq!(code, Some(1), "{out}");
        assert!(
            out.contains(&format!("{gitignore}: line 1 adds a .gitignore pattern")),
            "{out}"
        );
    }
    // Removing a negation hides what it let through; comments, blank lines
    // and removed patterns hide nothing.
    let repo = Repo::new("gitignore-negation");
    let base = repo.commit(&[(".gitignore", Some("*.yml\n!/.github/\ntarget/\n"))]);
    let head = repo.commit(&[(".gitignore", Some("*.yml\n# c\n\n"))]);
    let (code, out) = repo.check(&base, &head, "0");
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains(".gitignore: removes a .gitignore negation"),
        "{out}"
    );
    assert!(!out.contains("adds a .gitignore pattern"), "{out}");
    let head = repo.commit(&[(".gitignore", Some("*.yml\n!/.github/\n# c\n"))]);
    let (code, out) = repo.check(&base, &head, "0");
    assert_eq!(code, Some(0), "{out}");
}

#[test]
fn changed_checker_reference() {
    let (code, out) = pull_request("ref", &[(CALLER, Some("caller-changed.yml"))]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains("workflows.yml: changes a call of the workflows-check workflow"),
        "{out}"
    );
    // Removing the call is a change of it too.
    let (code, _) = pull_request("ref-removed", &[(CALLER, None)]);
    assert_eq!(code, Some(1));
    // So is turning it off with the same `uses`: a skipped required job
    // reports success.
    for fixture in ["caller-skipped.yml", "caller-untriggered.yml"] {
        let (code, out) = pull_request(fixture, &[(CALLER, Some(fixture))]);
        assert_eq!(code, Some(1), "{fixture}: {out}");
        assert!(out.contains("changes a call of the workflows"), "{out}");
    }
    // Another job in the file may change; making the call need it may not.
    let repo = Repo::new("ref-other");
    let caller = std::fs::read_to_string(Path::new(FIXTURES).join("caller.yml")).unwrap();
    let other = |caller: &str, run: &str| {
        format!("{caller}  other:\n    runs-on: ubuntu-26.04\n    steps: [{{run: '{run}'}}]\n")
    };
    let base = repo.commit(&[(CALLER, Some(&other(&caller, "a")))]);
    let head = repo.commit(&[(CALLER, Some(&other(&caller, "b")))]);
    let (code, out) = repo.check(&base, &head, "0");
    assert_eq!(code, Some(0), "{out}");
    let needs = caller.replace("  check:\n", "  check:\n    needs: other\n");
    let head = repo.commit(&[(CALLER, Some(&other(&needs, "b")))]);
    assert_eq!(repo.check(&base, &head, "0").0, Some(1));
}

#[test]
fn exemptions_come_from_the_base() {
    let repo = Repo::new("exempt");
    let exemptions = ".github/agentic-job/workflows.toml";
    let base = repo.commit(&[
        (CI, Some("base.yml")),
        (exemptions, Some("[exempt]\n\"ci.yml\" = \"base\"\n")),
    ]);
    let head = repo.commit(&[(exemptions, Some("[exempt]\n\"ci.yml\" = \"head\"\n"))]);
    let (code, out) = repo.check(&base, &head, "0");
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("= \"base\"") && !out.contains("head"), "{out}");
    // The shape check's refusal is the check's.
    assert_eq!(repo.check(&base, &head, "1").0, Some(1));
    assert_eq!(repo.check(&base, &head, "3").0, Some(2));
}

#[test]
fn bad_input() {
    let repo = Repo::new("bad");
    let base = repo.commit(&[(CI, Some("base.yml"))]);
    assert_eq!(repo.check(&base, "HEAD", "0").0, Some(2));
    assert_eq!(repo.check(&base[..12], &base, "0").0, Some(2));
    // A link where a workflow should be.
    std::os::unix::fs::symlink("/etc/passwd", repo.0.join(".github/workflows/link.yml")).unwrap();
    let head = repo.commit(&[]);
    assert_eq!(repo.check(&base, &head, "0").0, Some(2));
}
