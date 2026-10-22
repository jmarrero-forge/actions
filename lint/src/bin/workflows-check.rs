//! The check an organization ruleset requires of every pull request
//! (lint/workflows-check.md): a pull request's workflow files, read from
//! Git as blobs and never checked out or run, against the job shape and
//! rules about what a pull request may change.
//!
//! Usage: `workflows-check [--shape-check BINARY] REPOSITORY BASE HEAD`,
//! with BASE and HEAD full commit IDs. Exits 0 when nothing is refused, 1
//! when something is, 2 when the check could not run.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use yaml_rust2::{Yaml, YamlLoader};

/// Triggers that run with the base branch's token and secrets on an
/// event a pull request's author controls; a pull request may not add one.
const FORBIDDEN_TRIGGERS: &[&str] = &["pull_request_target", "workflow_run"];

/// zizmor's own configuration, which can disable an audit for the whole
/// repository as an ignore comment does for one line.
const ZIZMOR_CONFIGS: &[&str] = &[
    ".github/zizmor.yml",
    ".github/zizmor.yaml",
    "zizmor.yml",
    "zizmor.yaml",
];

/// actionlint's configuration, whose `paths` ignores can silence every
/// error in a workflow and `self-hosted-runner.labels` the unknown-label one.
const ACTIONLINT_CONFIGS: &[&str] = &[".github/actionlint.yaml", ".github/actionlint.yml"];

/// The shape check's exemptions, read from the base commit only.
const EXEMPTIONS: &str = ".github/agentic-job/workflows.toml";

/// This checker's own workflow, by file name: a repository may call it
/// too, and a pull request may not repoint that call.
const CHECKER_FILES: &[&str] = &["workflows-check.yml", "workflows-check.yaml"];

const CALL_CHANGED: &str = "changes a call of the workflows-check workflow: its job, a job it needs or the file's triggers";

const MAX_FILES: usize = 500;
const MAX_FILE: u64 = 1024 * 1024;
const MAX_EXEMPTIONS: u64 = 64 * 1024;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut shape = None;
    if args.first().map(String::as_str) == Some("--shape-check") && args.len() > 1 {
        shape = Some(PathBuf::from(args.remove(1)));
        args.remove(0);
    }
    let [repo, base, head] = args.as_slice() else {
        eprintln!("usage: workflows-check [--shape-check BINARY] REPOSITORY BASE HEAD");
        return ExitCode::from(2);
    };
    // The output quotes the pull request's file names and lines; stop the
    // runner from reading any of them as a workflow command.
    let token = (std::env::var_os("GITHUB_ACTIONS").is_some()).then(stop_token);
    if let Some(token) = &token {
        println!("::stop-commands::{token}");
    }
    let code = match run(Path::new(repo), base, head, shape) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            println!("workflows-check: {e}");
            ExitCode::from(2)
        }
    };
    if let Some(token) = &token {
        println!("::{token}::");
    }
    code
}

fn run(repo: &Path, base: &str, head: &str, shape: Option<PathBuf>) -> Result<bool, String> {
    for sha in [base, head] {
        if sha.len() != 40 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("not a full commit ID: {sha:?}"));
        }
    }
    let git = Git(repo);
    // What the pull request changed, as a review of it sees it.
    let ancestor = String::from_utf8_lossy(&git.run(&["merge-base", base, head])?)
        .trim()
        .to_string();
    let dir = TempDir::new()?;
    let mut ok = shape_check(&git, base, head, shape, &dir.0)?;
    let names = git.run(&[
        "diff",
        "--no-ext-diff",
        "--no-renames",
        "--name-only",
        "-z",
        &ancestor,
        head,
        "--",
    ])?;
    for name in names.split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let name = String::from_utf8_lossy(name);
        for problem in rules(&git, &ancestor, head, &name)? {
            println!("{name}: {problem}");
            ok = false;
        }
    }
    println!("workflows-check: {}", if ok { "passed" } else { "refused" });
    Ok(ok)
}

/// agentic-job's `workflows check` over the head's workflow files, with
/// the base's exemptions so that a pull request cannot exempt itself.
fn shape_check(
    git: &Git,
    base: &str,
    head: &str,
    shape: Option<PathBuf>,
    dir: &Path,
) -> Result<bool, String> {
    let root = dir.join("root");
    let workflows = root.join(".github/workflows");
    std::fs::create_dir_all(&workflows).map_err(|e| e.to_string())?;
    let tree = git.run(&["ls-tree", "-z", head, "--", ".github/workflows/"])?;
    let mut count = 0;
    for entry in tree.split(|&b| b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let (meta, path) = entry.split_once('\t').ok_or("unexpected ls-tree output")?;
        // GitHub reads no subdirectory, so neither does the shape check.
        if meta.split(' ').nth(1) == Some("tree") || !is_workflow(path) {
            continue;
        }
        count += 1;
        if count > MAX_FILES {
            return Err(format!("more than {MAX_FILES} workflow files"));
        }
        let data = git.blob(head, path, MAX_FILE)?.ok_or("vanished")?;
        let file = Path::new(path).file_name().ok_or("bad path")?;
        std::fs::write(workflows.join(file), data).map_err(|e| e.to_string())?;
    }
    let exemptions = dir.join("exemptions.toml");
    let data = git
        .blob(base, EXEMPTIONS, MAX_EXEMPTIONS)?
        .unwrap_or_default();
    std::fs::write(&exemptions, data).map_err(|e| e.to_string())?;
    let binary = match shape {
        Some(binary) => binary,
        None => fetch_shape_check(dir)?,
    };
    let status = Command::new(&binary)
        .args(["workflows", "check", "--root"])
        .arg(&root)
        .arg("--exemptions")
        .arg(&exemptions)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("{}: {e}", binary.display()))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("the shape check failed to run ({status})")),
    }
}

/// The binary lint/shape-check.yml names, refused unless its digest
/// matches, so a moved release tag cannot swap it.
fn fetch_shape_check(dir: &Path) -> Result<PathBuf, String> {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("shape-check.yml");
    let source =
        std::fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let doc = YamlLoader::load_from_str(&source).map_err(|e| e.to_string())?;
    let (Some(url), Some(sha256)) = (doc[0]["url"].as_str(), doc[0]["sha256"].as_str()) else {
        return Err(format!("{}: needs url and sha256", config.display()));
    };
    let binary = dir.join("shape-check");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
        ])
        .args(["--max-time", "300", "--retry", "3", "--output"])
        .arg(&binary)
        .arg(url)
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("downloading {url}: {status}"));
    }
    let out = Command::new("sha256sum")
        .arg(&binary)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    let digest = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || digest.split(' ').next() != Some(sha256) {
        return Err(format!("{url}: SHA-256 is not {sha256}"));
    }
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(binary)
}

/// The refusals for one file the pull request changed.
fn rules(git: &Git, ancestor: &str, head: &str, name: &str) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    if ZIZMOR_CONFIGS.contains(&name) {
        problems.push(
            "changes zizmor's configuration, which can disable an audit for the whole repository"
                .into(),
        );
        return Ok(problems);
    }
    if ACTIONLINT_CONFIGS.contains(&name) {
        problems.push(
            "changes actionlint's configuration, which can silence its errors for any workflow"
                .into(),
        );
        return Ok(problems);
    }
    // zizmor's default collection skips what a .gitignore at any depth
    // matches, tracked files included, so a pull request could add a
    // workflow with `git add -f` and the line that hides it from the audit.
    // The lint Justfile collects with `--collect=all`, but a caller's pin
    // of it may predate that.
    if Path::new(name).file_name().and_then(|n| n.to_str()) == Some(".gitignore") {
        let patch = patch(git, ancestor, head, name)?;
        let patch = String::from_utf8_lossy(&patch);
        for (number, line) in added_lines(&patch) {
            // Leading spaces are part of a pattern; unescaped trailing ones not.
            let line = line.trim_end();
            if !line.is_empty() && !line.starts_with('#') {
                problems.push(format!(
                    "line {number} adds a .gitignore pattern, which can hide a file from zizmor"
                ));
            }
        }
        // Removing a negation ignores again what it had let through.
        if removed_lines(&patch).iter().any(|l| l.starts_with('!')) {
            problems
                .push("removes a .gitignore negation, which can hide a file from zizmor".into());
        }
        return Ok(problems);
    }
    let workflow = is_workflow(name);
    // The other files zizmor audits: actions and Dependabot's configuration.
    let audited = matches!(
        Path::new(name).file_name().and_then(|n| n.to_str()),
        Some("action.yml" | "action.yaml")
    ) || matches!(name, ".github/dependabot.yml" | ".github/dependabot.yaml");
    if !workflow && !audited {
        return Ok(problems);
    }
    let old = if workflow {
        git.blob(ancestor, name, MAX_FILE)?
            .and_then(|old| parse(&String::from_utf8_lossy(&old)))
            .unwrap_or(Yaml::BadValue)
    } else {
        Yaml::BadValue
    };
    let Some(new) = git.blob(head, name, MAX_FILE)? else {
        // Deleted: only a call of the checker can be lost with it.
        if !checker_calls(&old).is_empty() {
            problems.push(CALL_CHANGED.into());
        }
        return Ok(problems);
    };
    let new = String::from_utf8_lossy(&new).into_owned();
    let patch = patch(git, ancestor, head, name)?;
    for (number, line) in added_lines(&String::from_utf8_lossy(&patch)) {
        if ignore_count(line) > 0 && !allowed_ignore(line) {
            problems.push(format!(
                "line {number} adds a zizmor ignore; only `# zizmor: ignore[self-repository]` on a `uses: ./...` line is allowed"
            ));
        }
    }
    if !workflow {
        return Ok(problems);
    }
    // A byte order mark at the start of any line is dropped by libyaml's
    // kind of parser and kept in the key by yaml-rust2's, here and maybe in
    // the shape check's, so `\u{feff}on:` can hide a trigger or a job.
    if new.contains('\u{feff}') {
        problems.push("contains a byte order mark, which YAML parsers read differently".into());
        return Ok(problems);
    }
    let Some(new) = parse(&new) else {
        problems.push("cannot be parsed as YAML".into());
        return Ok(problems);
    };
    let before = triggers(&old);
    for trigger in triggers(&new) {
        if FORBIDDEN_TRIGGERS.contains(&trigger.as_str()) && !before.contains(&trigger) {
            problems.push(format!("adds the trigger `{trigger}`, which runs with the base branch's token on input the author controls"));
        }
    }
    if checker_calls(&old) != checker_calls(&new) {
        problems.push(CALL_CHANGED.into());
    }
    Ok(problems)
}

/// `.github/workflows/NAME.yml` or `.yaml`, in no subdirectory.
fn is_workflow(path: &str) -> bool {
    let Some(file) = path.strip_prefix(".github/workflows/") else {
        return false;
    };
    !file.contains('/') && runner_labels::has_workflow_extension(file)
}

fn parse(source: &str) -> Option<Yaml> {
    // yaml-rust2 keeps a byte order mark as part of the first key, where
    // GitHub drops it: `\u{feff}on:` would hide the triggers.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut docs = YamlLoader::load_from_str(source).ok()?;
    (docs.len() == 1).then(|| docs.remove(0))
}

/// The event names under `on`: a string, a list or a mapping.
fn triggers(doc: &Yaml) -> Vec<String> {
    let on = &doc["on"];
    let names: Vec<&Yaml> = match on {
        Yaml::Hash(map) => map.keys().collect(),
        Yaml::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    names
        .into_iter()
        .filter_map(Yaml::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// What decides whether a file's call of this checker runs, empty when it
/// has none: the file's `on`, then every job whose or whose step's `uses`
/// names the checker's workflow file, whole (`if`, `needs`, `with` and
/// the rest). Comparing only the `uses` string would let an `if: false` or
/// a trigger that never fires turn the call off, and a skipped required
/// job reports success. The jobs it `needs` are not compared: they can be
/// made to fail through files no YAML comparison sees (workflows-check.md).
fn checker_calls(doc: &Yaml) -> Vec<(String, Yaml)> {
    let Some(jobs) = doc["jobs"].as_hash() else {
        return Vec::new();
    };
    let calls: Vec<(String, Yaml)> = jobs
        .iter()
        .filter(|(_, job)| {
            let steps = job["steps"].as_vec().into_iter().flatten();
            std::iter::once(&job["uses"])
                .chain(steps.map(|s| &s["uses"]))
                .filter_map(Yaml::as_str)
                .any(|uses| {
                    let path = uses
                        .split('@')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    CHECKER_FILES.contains(&path.rsplit('/').next().unwrap_or_default())
                })
        })
        .map(|(id, job)| (id.as_str().unwrap_or("?").to_string(), job.clone()))
        .collect();
    if calls.is_empty() {
        return calls;
    }
    std::iter::once(("on".to_string(), doc["on"].clone()))
        .chain(calls)
        .collect()
}

/// One file's change as a `-U0` patch, its content as text whatever it is.
fn patch(git: &Git, ancestor: &str, head: &str, name: &str) -> Result<Vec<u8>, String> {
    git.run(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--text",
        "--no-renames",
        "-U0",
        ancestor,
        head,
        "--",
        name,
    ])
}

/// The lines a `-U0` patch removes, those after its first hunk header so
/// that the `---` file header is not one.
fn removed_lines(patch: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut in_hunk = false;
    for line in patch.split('\n') {
        if line.starts_with("@@ ") {
            in_hunk = true;
        } else if in_hunk && let Some(removed) = line.strip_prefix('-') {
            lines.push(removed);
        }
    }
    lines
}

/// The lines a `-U0` patch adds, with their numbers in the new file.
fn added_lines(patch: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut number = 0;
    for line in patch.split('\n') {
        if let Some(hunk) = line.strip_prefix("@@ ") {
            // @@ -a,b +c,d @@
            number = hunk
                .split(' ')
                .nth(1)
                .and_then(|new| new.trim_start_matches('+').split(',').next()?.parse().ok())
                .unwrap_or(0);
        } else if number > 0
            && let Some(added) = line.strip_prefix('+')
        {
            lines.push((number, added));
            number += 1;
        }
    }
    lines
}

/// Occurrences of `zizmor: ignore`, looser than zizmor's own syntax
/// (any case, space around the colon) so that a near miss counts too.
fn ignore_count(line: &str) -> usize {
    let lower = line.to_ascii_lowercase();
    let mut count = 0;
    for (at, _) in lower.match_indices("zizmor") {
        let rest = lower[at + "zizmor".len()..].trim_start();
        if let Some(rest) = rest.strip_prefix(':')
            && rest.trim_start().starts_with("ignore")
        {
            count += 1;
        }
    }
    count
}

/// The one ignore a pull request may add, as agentic-job's ingress check
/// allows it: `self-repository` on a `uses:` of a local action or
/// workflow, which zizmor flags for every `./` path. It is matched as the
/// whole line rather than by YAML structure; misplaced, it can still only
/// silence that one audit on that one line.
fn allowed_ignore(line: &str) -> bool {
    if ignore_count(line) != 1 {
        return false;
    }
    let line = line.trim_start_matches(' ');
    let line = line.strip_prefix("- ").unwrap_or(line);
    let Some(value) = line.strip_prefix("uses: ") else {
        return false;
    };
    // The path, bare or in quotes of one kind with none inside.
    let (path, rest) = match value.chars().next() {
        Some(q @ ('\'' | '"')) => match value[1..].split_once(q) {
            Some((path, rest)) => (path, rest),
            None => return false,
        },
        _ => {
            let end = value.find(' ').unwrap_or(value.len());
            let path = &value[..end];
            if path.contains(['#', '\'', '"']) {
                return false;
            }
            (path, &value[end..])
        }
    };
    if !path.starts_with("./") || path.contains("..") || path.contains(['\r', '\n']) {
        return false;
    }
    let Some(comment) = rest
        .trim_start_matches(' ')
        .strip_prefix("# zizmor: ignore[self-repository]")
    else {
        return false;
    };
    rest.starts_with(' ') && (comment.is_empty() || comment.starts_with(' '))
}

struct Git<'a>(&'a Path);

impl Git<'_> {
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let out = Command::new("git")
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(self.0)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(out.stdout)
    }

    /// A regular file's contents at a commit, None when there is no such
    /// path; a link, a submodule or a file over `max` bytes is an error.
    fn blob(&self, commit: &str, path: &str, max: u64) -> Result<Option<Vec<u8>>, String> {
        let entry = self.run(&["ls-tree", "-z", commit, "--", path])?;
        let entry = String::from_utf8_lossy(&entry);
        let Some((meta, _)) = entry.split_once('\t') else {
            return Ok(None);
        };
        let [mode, kind, oid] = meta.split(' ').collect::<Vec<_>>()[..] else {
            return Err(format!("{path}: unexpected ls-tree output"));
        };
        if kind != "blob" || !matches!(mode, "100644" | "100755") {
            return Err(format!("{path}: not a regular file"));
        }
        let size: u64 = String::from_utf8_lossy(&self.run(&["cat-file", "-s", oid])?)
            .trim()
            .parse()
            .map_err(|_| "bad size")?;
        if size > max {
            return Err(format!("{path}: over {max} bytes"));
        }
        self.run(&["cat-file", "blob", oid]).map(Some)
    }
}

/// A fresh directory for this run, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, String> {
        let base = std::env::var_os("RUNNER_TEMP").map_or_else(std::env::temp_dir, PathBuf::from);
        let path = base.join(format!(
            "workflows-check-{}-{}",
            std::process::id(),
            stop_token()
        ));
        std::fs::create_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 32 random hex digits.
fn stop_token() -> String {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .expect("/dev/urandom");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores() {
        for line in [
            "      - uses: ./local # zizmor: ignore[self-repository]",
            "    uses: './.github/workflows/x.yml' # zizmor: ignore[self-repository] reason",
            "uses: \"./a b\" # zizmor: ignore[self-repository]",
        ] {
            assert!(allowed_ignore(line), "{line}");
        }
        for line in [
            "on: pull_request_target # zizmor: ignore[dangerous-triggers]",
            "      - uses: ./local # zizmor: ignore[self-repository,unpinned-uses]",
            "      - uses: ./local # zizmor: ignore[self-repository] # zizmor: ignore[x]",
            "      - uses: ./local #zizmor: ignore[self-repository]",
            "      - uses: ../up # zizmor: ignore[self-repository]",
            "      - uses: org/repo@main # zizmor: ignore[self-repository]",
            "      - run: x # zizmor: ignore[self-repository]",
            "      - uses: ./local# zizmor: ignore[self-repository]",
            "      - uses: './x # zizmor: ignore[self-repository]",
        ] {
            assert!(!allowed_ignore(line), "{line}");
        }
        assert_eq!(ignore_count("# ZIZMOR : Ignore[x]"), 1);
        assert_eq!(ignore_count("# zizmor ignore"), 0);
    }

    #[test]
    fn trigger_forms() {
        let t = |s: &str| triggers(&parse(s).unwrap());
        assert_eq!(t("on: workflow_run\n"), ["workflow_run"]);
        assert_eq!(
            t("on: [push, Pull_Request_Target]\n"),
            ["push", "pull_request_target"]
        );
        assert_eq!(
            t("on:\n  push:\n  workflow_run: {workflows: [x]}\n"),
            ["push", "workflow_run"]
        );
        assert!(t("jobs: {}\n").is_empty());
    }

    #[test]
    fn hunks() {
        let patch = "diff --git a/x b/x\n+++ b/x\n@@ -1,0 +2,2 @@\n+a\n+b\n@@ -9 +12 @@\n-c\n+d\n";
        assert_eq!(added_lines(patch), [(2, "a"), (3, "b"), (12, "d")]);
        let patch = "--- a/x\n+++ b/x\n@@ -1 +0,0 @@\n--x\n@@ -5 +4 @@\n-!y\n+z\n";
        assert_eq!(removed_lines(patch), ["-x", "!y"]);
    }

    #[test]
    fn checker_references() {
        let doc = parse(
            "on: push\njobs:\n  a: {uses: o/r/.github/workflows/workflows-check.yml@v1, needs: b}\n  b: {steps: [{uses: actions/checkout@v1}]}\n  c: {steps: [{uses: ./.github/workflows/Workflows-Check.yaml}]}\n",
        )
        .unwrap();
        let names: Vec<_> = checker_calls(&doc).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["on", "a", "c"]);
        assert!(checker_calls(&parse("on: push\njobs: {c: {}}\n").unwrap()).is_empty());
    }

    #[test]
    fn workflow_paths() {
        assert!(is_workflow(".github/workflows/a.YML"));
        assert!(!is_workflow(".github/workflows/sub/a.yml"));
        assert!(!is_workflow("lint/a.yml"));
    }
}
