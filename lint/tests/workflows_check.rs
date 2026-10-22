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
