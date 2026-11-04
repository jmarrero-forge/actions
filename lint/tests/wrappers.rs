//! The wrapper workflows' structure, read as text from .github/workflows,
//! and the few steps of theirs that are shell, run through bash. The tests
//! that execute the target job's github-script step or Renovate's regular
//! expressions, and those about inference credentials, stay in
//! cgwalters-agentic-job.test.mjs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use yaml_rust2::{Yaml, YamlLoader};

const AGENTIC_JOB: &str = ".github/workflows/cgwalters-agentic-job.yml";
const REVIEW: &str = ".github/workflows/cgwalters-review.yml";
const REVIEW_CALLER: &str = ".github/workflows/review.yml";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn read(path: &str) -> String {
    let path = root().join(path);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// One whole line of TEXT at INDENT, with the newlines around it.
fn line(indent: usize, text: &str) -> String {
    format!("\n{}{text}\n", " ".repeat(indent))
}

/// The text after each PREFIX up to the newline that ends its line, the
/// next search starting after that newline.
fn captures<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(prefix) {
        let after = &rest[start + prefix.len()..];
        let Some(end) = after.find('\n') else { break };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out
}

/// Whether some line ending with BEFORE is followed by any one line and
/// then by AFTER, which starts with a newline.
fn line_then(text: &str, before: &str, after: &str) -> bool {
    text.match_indices(before).any(|(i, _)| {
        let rest = &text[i + before.len()..];
        rest.find('\n')
            .is_some_and(|end| rest[end..].starts_with(after))
    })
}

/// The first line, after comments and without its indent, of each `steps:`
/// at indent 4.
fn first_steps(text: &str) -> Vec<&str> {
    let steps = line(4, "steps:");
    text.match_indices(&steps)
        .filter_map(|(i, _)| {
            let mut rest = &text[i + steps.len()..];
            while rest.starts_with("      #")
                && let Some(end) = rest.find('\n')
            {
                rest = &rest[end + 1..];
            }
            let (first, _) = rest.strip_prefix("      ")?.split_once('\n')?;
            Some(first)
        })
        .collect()
}

/// Whether some `steps:` at indent 4 begins, after comments, with FIRST.
fn steps_begin_with(text: &str, first: &str) -> bool {
    first_steps(text).contains(&first)
}

fn is_commit(r: &str) -> bool {
    r.len() == 40 && r.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether R, what follows a `uses:` line's `@`, is a full commit marked as
/// taken from main.
fn main_pin(r: &str) -> bool {
    r.strip_suffix(" # main").is_some_and(is_commit)
}

/// Each (action, ref) of TEXT's calls of this repository's actions.
fn own_action_refs(text: &str) -> Vec<(&str, &str)> {
    text.match_indices("uses: jmarrero-forge/actions/")
        .filter_map(|(i, prefix)| {
            let rest = &text[i + prefix.len()..];
            let token = &rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())];
            token
                .rsplit_once('@')
                .filter(|(path, r)| !path.is_empty() && !r.is_empty())
        })
        .collect()
}

/// Lines without comments, blank lines or zizmor's inline ignores.
fn code(text: &str) -> String {
    text.split('\n')
        .map(|line| line.find(" # zizmor: ").map_or(line, |i| &line[..i]))
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The offsets of each `KEY:` line at INDENT that follows a newline.
fn key_starts(text: &str, indent: usize) -> Vec<usize> {
    text.match_indices('\n')
        .map(|(i, _)| i + 1 + indent)
        .filter(|&start| {
            let line = &text[start - indent..];
            let Some(rest) = line.strip_prefix(&" ".repeat(indent)) else {
                return false;
            };
            let key = rest
                .bytes()
                .take_while(|b| b.is_ascii_lowercase() || *b == b'-')
                .count();
            key > 0 && rest[key..].starts_with(":\n")
        })
        .collect()
}

/// TEXT up to its first `KEY:` line at INDENT.
fn until_key(text: &str, indent: usize) -> &str {
    key_starts(text, indent)
        .first()
        .map_or(text, |&start| &text[..start - indent - 1])
}

/// The text of one job, from its key to the next job's.
fn job(workflow: &str, name: &str) -> String {
    let after = workflow
        .split(&format!("\n  {name}:\n"))
        .nth(1)
        .unwrap_or_else(|| panic!("no job {name}"));
    format!("\n{}", until_key(after, 2))
}

struct Job {
    body: String,
    needs: Vec<String>,
}

// Any public repository can call the wrappers, and the tokens of its run
// carry their `job_workflow_ref`; the agent job runs on a runner the caller
// picks, whose owner could mint tokens for any audience there. So every
// job waits for the admit action, run first on a hosted runner.
fn needs_graph(text: &str) -> BTreeMap<String, Job> {
    let section = text.split("\njobs:\n").nth(1).unwrap();
    let mut keys = Vec::new();
    let mut offset = 0;
    for line in section.split_inclusive('\n') {
        if let Some(key) = line
            .trim_end_matches('\n')
            .strip_prefix("  ")
            .and_then(|l| l.strip_suffix(':'))
            && !key.is_empty()
            && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            keys.push((offset, key.to_owned()));
        }
        offset += line.len();
    }
    keys.iter()
        .enumerate()
        .map(|(index, (start, name))| {
            let end = keys.get(index + 1).map_or(section.len(), |k| k.0);
            let body = section[*start..end].to_owned();
            let needs = captures(&body, "\n    needs: ")
                .into_iter()
                .find(|n| {
                    (n.starts_with('[') && n.ends_with(']') && n.find(']') == Some(n.len() - 1))
                        || (!n.is_empty() && !n.contains(char::is_whitespace))
                })
                .map_or(Vec::new(), |n| {
                    n.replace(['[', ']'], "")
                        .split(',')
                        .map(|need| need.trim().to_owned())
                        .filter(|need| !need.is_empty())
                        .collect()
                });
            (name.clone(), Job { body, needs })
        })
        .collect()
}

fn admits_first(text: &str, gate: &str) {
    let graph = needs_graph(text);
    let body = &graph[gate].body;
    assert!(
        captures(body, "\n    runs-on: ubuntu-")
            .iter()
            .any(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')),
        "{gate} runs on a hosted runner"
    );
    assert!(
        first_steps(body).iter().any(|step| step
            .strip_prefix("- uses: jmarrero-forge/actions/admit@")
            .is_some_and(|r| r == "main" || main_pin(r))),
        "{gate} begins with the admit action"
    );
    assert_eq!(graph[gate].needs.len(), 0);
    assert!(!body.contains("\n    if:"));
    fn reaches(graph: &BTreeMap<String, Job>, gate: &str, name: &str) -> bool {
        name == gate || graph[name].needs.iter().any(|n| reaches(graph, gate, n))
    }
    for name in graph.keys() {
        assert!(reaches(&graph, gate, name), "{name} waits for {gate}");
    }
}

struct TempDir(PathBuf);

impl TempDir {
    /// A new directory under PARENT.
    fn new(parent: &Path, prefix: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = parent.join(format!(
            "{prefix}-{}-{}",
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

// issues runs with a write scope on input an outsider
// writes: safe only while nothing checks out, reads or runs the item. So the
// workflow is pinned whole, comments aside: one job, one API call, and only
// the item's number, never its title, body or head, in the step.
#[test]
fn lock_new_items_only_locks_newly_opened_issues_by_number() {
    assert_eq!(
        code(&read(".github/workflows/lock-new-items.yml")),
        r#"name: lock-new-items
on:
  workflow_call:
permissions: {}
jobs:
  lock:
    if: >-
      github.event.action == 'opened' &&
      github.event_name == 'issues'
    runs-on: ubuntu-26.04
    timeout-minutes: 5
    permissions:
      issues: write
    steps:
      - name: Lock the item
        env:
          GH_TOKEN: ${{ github.token }}
          REPO: ${{ github.repository }}
          ITEM: ${{ github.event.issue.number }}
        run: gh api --method PUT "repos/$REPO/issues/$ITEM/lock" --silent"#
    );
}

// The caller: only issues opened, only the issues grant, no secret, and the
// local reference, which resolves on the base branch.
fn lock_caller(uses: &str) -> String {
    format!(
        "name: lock
on:
  issues:
    types: [opened]
permissions: {{}}
jobs:
  lock:
    uses: {uses}
    permissions:
      issues: write"
    )
}

#[test]
fn lock_is_called_from_the_base_branch_with_no_secret() {
    assert_eq!(
        code(&read(".github/workflows/lock.yml")),
        lock_caller("./.github/workflows/lock-new-items.yml")
    );
}

// The one file the other repositories copy is this repository's caller,
// at main.
#[test]
fn lock_documented_caller_is_this_one_called_at_main() {
    let readme = read("README.md");
    let block = readme
        .split("\n## Lock new items\n")
        .nth(1)
        .and_then(|docs| docs.split_once("\n```yaml\n"))
        .and_then(|(_, rest)| rest.split_once("\n```\n"))
        .map(|(block, _)| block);
    let block = block.expect("README.md documents the caller");
    assert_eq!(
        code(block),
        lock_caller("cgwalters-forge/actions/.github/workflows/lock-new-items.yml@main")
    );
}

// The review gate: a thin caller of this repository's cgwalters-review.yml,
// with this repository's copies of the bounds and the task, which that
// workflow reads from the base branch (agentic-job's docs/review.md,
// "Another repository").
//
// pull_request_target runs with write scopes: safe only while nothing here
// checks out or runs the head and no secret is handed down. So the caller
// is pinned whole, comments aside: one job, no steps, no secrets, and only
// the triggers and grants agentic-job's docs/review.md gives a caller. The
// `if:` admits every pull request event and `/review` comments; the called
// workflow narrows both (authors, and who may comment).
#[test]
fn review_caller_runs_nothing_of_the_head_and_passes_no_secret() {
    assert_eq!(
        code(&read(REVIEW_CALLER)),
        "name: review
on:
  pull_request_target:
    types: [opened, synchronize, reopened, ready_for_review]
  issue_comment:
    types: [created]
permissions: {}
jobs:
  review:
    if: >-
      github.event_name == 'pull_request_target' ||
      (github.event.issue.pull_request && startsWith(github.event.comment.body, '/review'))
    uses: ./.github/workflows/cgwalters-review.yml
    permissions:
      contents: read
      issues: read
      pull-requests: write
      actions: read
      id-token: write
      statuses: write"
    );
}

// Moving main must not change privileged review code behind a caller's
// reviewed wrapper pin. Check every own-repository call, not just tailscale.
#[test]
fn review_wrapper_pins_its_own_actions_by_full_commit() {
    let text = read(REVIEW);
    let refs = own_action_refs(&text);
    assert_eq!(
        refs.iter().map(|r| r.0).collect::<Vec<_>>(),
        ["admit", "deployment", "tailscale"]
    );
    for (action, r) in refs {
        assert!(is_commit(r), "{action}: {r}");
    }
}

// Unlike upstream's, our dispatch wrapper pins them too: jmarrero's broker
// admits it at the commit its caller pins.
#[test]
fn dispatch_wrapper_pins_its_own_actions_by_full_commit() {
    let text = read(AGENTIC_JOB);
    let refs = own_action_refs(&text);
    assert_eq!(
        refs.iter().map(|r| r.0).collect::<Vec<_>>(),
        ["admit", "deployment", "tailscale"]
    );
    for (action, r) in refs {
        assert!(is_commit(r), "{action}: {r}");
    }
}

// From v0.5.0 check.yml's detect job asks for `id-token: write`, and GitHub
// refuses a call that grants less than a called job asks for: every run
// would fail at startup. From v0.6.2 its check job asks for `actions: read`,
// to verify the proposals upload's digest. Nothing more than that, as no
// secret goes to check.
#[test]
fn both_wrappers_grant_check_exactly_what_its_jobs_ask_for() {
    let grant = "\n    permissions:\n      contents: read\n      actions: read\n      id-token: write\n    ";
    for file in [AGENTIC_JOB, REVIEW] {
        let text = read(file);
        let check = until_key(text.split("\n  check:\n").nth(1).unwrap(), 2);
        let granted = check.match_indices(grant).any(|(i, _)| {
            let mut rest = &check[i + grant.len()..];
            while rest.starts_with('#')
                && let Some(end) = rest.find('\n')
                && rest[end + 1..].starts_with("    ")
            {
                rest = &rest[end + 5..];
            }
            rest.starts_with("uses: ")
        });
        assert!(granted, "{file}");
        assert!(!check.contains("secrets"), "{file}");
    }
}

/// Each (scope, level) of every `permissions:` block at indent 4.
fn grants(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, prefix) in text.match_indices("\n    permissions:\n") {
        for line in text[i + prefix.len()..].split('\n') {
            let Some(grant) = line.strip_prefix("      ") else {
                break;
            };
            if grant.is_empty() || grant.starts_with(char::is_whitespace) {
                break;
            }
            let mut parts = line.trim().split(": ");
            let scope = parts.next().unwrap_or_default().to_owned();
            let level = parts.next().unwrap_or_default().to_owned();
            out.push((scope, level));
        }
    }
    out
}

// A called workflow's jobs get at most what the caller grants, so the caller
// grants what the wrapper's jobs ask for, and nothing more.
#[test]
fn review_caller_grants_exactly_the_union_of_the_wrappers_job_grants() {
    let mut union = BTreeMap::new();
    for (scope, level) in grants(&read(REVIEW)) {
        if union.get(&scope).map(String::as_str) != Some("write") {
            union.insert(scope, level);
        }
    }
    let caller: BTreeMap<_, _> = grants(&read(REVIEW_CALLER)).into_iter().collect();
    assert_eq!(caller, union);
}

// checkout reads an empty `repository` as the calling repository and an
// empty `ref` as its default branch, so a job that runs the policy call's
// source checks first that policy reported the repository it calls, at a
// full commit. Every such job, so that a recomposition cannot drop it.
#[test]
fn every_job_checks_the_policy_calls_source_before_checking_it_out() {
    const CHECKOUT: &str = "repository: ${{ needs.policy.outputs.source-repository }}";
    for file in [AGENTIC_JOB, REVIEW] {
        let workflow = read(file);
        let called = workflow
            .match_indices("\n    uses: ")
            .map(|(i, p)| &workflow[i + p.len()..])
            .map(|rest| &rest[..rest.find('\n').unwrap_or(rest.len())])
            .find_map(|rest| {
                let (repo, _) = rest.split_once("/.github/workflows/policy.yml@")?;
                let (owner, name) = repo.split_once('/')?;
                (!owner.is_empty()
                    && !name.is_empty()
                    && !name.contains('/')
                    && !repo.contains(char::is_whitespace))
                .then_some(repo)
            })
            .unwrap_or_else(|| panic!("{file}: policy call"));
        let starts = key_starts(&workflow, 2);
        let jobs: Vec<_> = starts
            .iter()
            .enumerate()
            .map(|(index, &start)| {
                let end = starts
                    .get(index + 1)
                    .map_or(workflow.len(), |&next| next - 3);
                &workflow[start..end]
            })
            .collect();
        let checked: Vec<_> = jobs.iter().filter(|job| job.contains(CHECKOUT)).collect();
        assert_eq!(
            checked
                .iter()
                .map(|job| job.split(':').next().unwrap())
                .collect::<Vec<_>>(),
            ["agent"],
            "{file}: jobs checking out the source"
        );
        for job in checked {
            let name = job.split(':').next().unwrap();
            let steps: Vec<_> = job.split("\n      - ").collect();
            let checkout = steps
                .iter()
                .position(|step| step.contains(CHECKOUT))
                .unwrap();
            let guard = steps[..checkout]
                .iter()
                .find(|step| step.starts_with("name: The policy call's source repository"));
            let guard = guard.unwrap_or_else(|| panic!("{file}: {name} checks the source first"));
            assert!(
                guard.contains(&line(
                    10,
                    "SOURCE_REPOSITORY: ${{ needs.policy.outputs.source-repository }}"
                )),
                "{file}: {name}"
            );
            assert!(
                guard.contains(&line(
                    10,
                    "SOURCE_SHA: ${{ needs.policy.outputs.source-sha }}"
                )),
                "{file}: {name}"
            );
            let expected = captures(guard, "\n          EXPECTED_REPOSITORY: ")
                .into_iter()
                .next()
                .filter(|v| !v.is_empty() && !v.contains(char::is_whitespace));
            assert_eq!(expected, Some(called), "{file}: {name}");
            assert!(
                guard.contains(
                    "[[ \"$SOURCE_REPOSITORY\" == \"$EXPECTED_REPOSITORY\" && \"$SOURCE_SHA\" =~ ^[0-9a-f]{40}$ ]] ||\n            { echo \"::error::the policy call is not $EXPECTED_REPOSITORY at a full commit\"; exit 1; }"
                ),
                "{file}: {name}"
            );
        }
    }
}

#[test]
fn composed_review_links_its_pinned_helper_to_the_agent_job() {
    let workflow = read(REVIEW);
    let agent = workflow
        .split("\n  agent:\n")
        .nth(1)
        .unwrap()
        .split("\n  check:\n")
        .next()
        .unwrap();
    let submit = workflow
        .split("\n  submit:\n")
        .nth(1)
        .unwrap()
        .split("\n  apply:\n")
        .next()
        .unwrap();
    assert!(agent.contains(&line(6, "job: ${{ job.check_run_id }}")));
    assert!(submit.contains(&line(10, "AGENT_JOB: ${{ needs.agent.outputs.job }}")));
}

// The bounds, comments aside: comment-only outputs for this repository's
// main, automatic events for any role (the shared workflow's `if:` narrows
// them to owner, members and collaborators), `/review` only from writers.
fn bounds(name: &str) -> String {
    read(&format!("workflow/{name}.toml"))
        .split('\n')
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn common(repos: &str) -> String {
    format!(
        r#"hosts = ["github.com"]
repos = {repos}
bases = ["main"]
max_outputs = 1
max_patch_bytes = 1024
max_patch_files = 1
[github.outputs]
add_comment = {{ max = 1 }}
noop = {{ max = 1 }}
[github.trigger]"#
    )
}

/// Whether REPOS is a list of exactly one OWNER/NAME.
fn one_repository(repos: &str) -> bool {
    let name = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    repos
        .strip_prefix("[\"")
        .and_then(|r| r.strip_suffix("\"]"))
        .and_then(|r| r.split_once('/'))
        .is_some_and(|(owner, repo)| name(owner) && name(repo))
}

#[test]
fn review_bounds_admit_one_comment_on_this_repository_only() {
    let review = bounds("review");
    let command = bounds("review-command");
    let repos = review
        .lines()
        .filter_map(|line| line.strip_prefix("repos = "))
        .find(|repos| one_repository(repos))
        .expect("repos names exactly one repository");
    assert_eq!(
        review,
        format!(
            r#"{}
events = ["pull_request_target", "pull_request"]
roles = ["all"]
bots = ["dependabot[bot]"]
forks = true"#,
            common(repos)
        )
    );
    assert_eq!(
        command,
        format!(
            r#"{}
events = ["issue_comment"]
roles = ["admin", "maintain", "write"]
bots = []
forks = true
commands = ["/review"]"#,
            common(repos)
        )
    );
}

// agentic-job's own task at the called tag, not agent-board's: that one says
// no person approves after the review, but whether one does here is the
// owner's merge policy (.github/agentic-job/merge.toml, `person` without
// it), which the run appends to the task.
#[test]
fn review_task_treats_the_head_as_hostile_and_defers_to_the_merge_policy() {
    let task = read("workflow/review.md");
    for contract in [
        "hostile",
        "Do not modify code",
        "exact admitted head SHA",
        "VERDICT: APPROVE",
        "VERDICT: CHANGES",
        "stated under \"Merge policy\"",
        "Your output is only a comment",
    ] {
        assert!(task.contains(contract), "{contract}");
    }
    for claim in [
        "no person approves after you",
        "the merge gate for every change here",
    ] {
        assert!(!task.contains(claim), "{claim}");
    }
}

// agentic-job is called at one release: check, apply, prepare and run
// refuse calls that resolved to different commits, which following main
// produced for runs spanning an agentic-job merge (tracker#544).
// workflow/review.test.cjs checks that release's pin. Here our own admit,
// deployment and tailscale actions are pinned at a commit of main.
#[test]
fn agentic_job_is_called_at_one_release_our_own_actions_at_a_commit() {
    let workflow = read(AGENTIC_JOB);
    // Our own actions are this copy's, agentic-job stays upstream's.
    let refs: Vec<_> = captures(&workflow, "uses: ")
        .into_iter()
        .filter_map(|rest| {
            rest.strip_prefix("jmarrero-forge/")
                .filter(|r| r.starts_with("actions/"))
                .or_else(|| {
                    rest.strip_prefix("cgwalters-forge/")
                        .filter(|r| r.starts_with("agentic-job/"))
                })
        })
        .filter_map(|rest| {
            let (token, comment) = rest
                .split_once(' ')
                .map_or((rest, None), |(t, c)| (t, Some(c)));
            if let Some(comment) = comment {
                let tag = comment.strip_prefix("# ")?;
                if tag.is_empty() || tag.contains(char::is_whitespace) {
                    return None;
                }
            }
            token
                .rsplit_once('@')
                .filter(|(path, r)| !path.is_empty() && !r.is_empty())
        })
        .collect();
    assert_eq!(
        refs.iter().map(|r| r.0).collect::<Vec<_>>(),
        [
            "actions/admit",
            "actions/deployment",
            "agentic-job/.github/workflows/policy.yml",
            "actions/tailscale",
            "agentic-job/.github/workflows/check.yml",
            "agentic-job/.github/workflows/apply.yml",
        ]
    );
    for (path, r) in &refs {
        if path.starts_with("actions/") {
            assert!(is_commit(r), "{path}: {r}");
        }
    }
    assert!(!workflow.contains("PINNED_SHA"));
    assert!(workflow.contains(&line(
        10,
        "EXPECTED_REPOSITORY: cgwalters-forge/agentic-job"
    )));
}

// Without `issue`, apply opens an implement pull request with no closing
// reference, so its merge leaves the issue open (agentic-job#877), and
// policy selects no watcher for opt-in steering. Only the dispatched item,
// never another target: a merge must not close a tracking issue. Only
// implement opens a new pull request, so the other kinds close nothing.
#[test]
fn policy_is_given_the_dispatched_item_as_issue_and_nothing_else() {
    let workflow = read(AGENTIC_JOB);
    assert_eq!(
        captures(&workflow, "\n      issue: "),
        ["${{ inputs.item }}"]
    );
    assert!(job(&workflow, "policy").contains("\n      issue: "));
    // With the item in `repo` itself apply writes `Closes`; an `item-repo`
    // would make it a mere `Refs`.
    assert!(!workflow.contains("item-repo:"));
}

// Dependabot moves the tag, all three calls in one pull request, without
// the third-party cooldown.
#[test]
fn dependabot_proposes_new_agentic_job_tags_as_one_group() {
    let config = read(".github/dependabot.yml");
    assert!(config.contains(&line(6, "interval: daily")));
    assert!(
        config.contains(
            "\n      agentic-job:\n        patterns: [\"cgwalters-forge/agentic-job*\"]\n"
        )
    );
    assert!(config.contains(&line(6, "exclude: [\"cgwalters-forge/agentic-job*\"]")));
}

// A second deployment's copy sets its configuration repository in
// deployment/deployment.json, not in the wrapper.
#[test]
fn configuration_repository_defaults_to_deployment_json_after_the_input_and_the_variable() {
    let workflow = read(AGENTIC_JOB);
    assert!(!workflow.contains("agenthome"));
    assert!(workflow.contains(&line(
        10,
        "AGENT_CONFIG_REPO: ${{ inputs.agent-config-repo || vars.AGENT_CONFIG_REPO || steps.deployment.outputs.agent-config-repo }}"
    )));
    let target = job(&workflow, "target");
    let deployment = "\n      - uses: jmarrero-forge/actions/deployment@";
    assert!(target.match_indices(deployment).any(|(i, _)| {
        target[i + deployment.len()..]
            .split_once('\n')
            .is_some_and(|(r, rest)| main_pin(r) && rest.starts_with("        id: deployment\n"))
    }));
    let action = read("deployment/action.yml");
    assert!(line_then(
        &action,
        &line(2, "agent-config-repo:"),
        &line(4, "value: ${{ steps.read.outputs.agent-config-repo }}")
    ));
}

// Likewise the model, in both wrappers; the review wrapper has no steps
// before policy, so its admit job reads it.
#[test]
fn model_defaults_to_deployment_json_after_the_input_and_the_variable() {
    let workflow = read(AGENTIC_JOB);
    let review = read(REVIEW);
    let deployment = YamlLoader::load_from_str(&read("deployment/deployment.json")).unwrap();
    let model = deployment[0]["agent-model"]
        .as_str()
        .expect("deployment.json sets agent-model");
    for text in [&workflow, &review] {
        assert!(!text.contains(model), "{model}");
    }
    assert!(workflow.contains(&line(
        10,
        "AGENT_MODEL: ${{ inputs.model || vars.AGENT_MODEL || steps.deployment.outputs.agent-model }}"
    )));
    let admit = &needs_graph(&review)["admit"].body;
    assert!(admit.contains(&line(
        6,
        "agent-model: ${{ steps.deployment.outputs.agent-model }}"
    )));
    let deployment = "\n      - uses: jmarrero-forge/actions/deployment@";
    assert!(admit.match_indices(deployment).any(|(i, _)| {
        admit[i + deployment.len()..]
            .split_once('\n')
            .is_some_and(|(r, rest)| main_pin(r) && rest.starts_with("        id: deployment\n"))
    }));
    assert!(review.contains(&line(
        6,
        "model: ${{ vars.AGENT_MODEL || needs.admit.outputs.agent-model }}"
    )));
    let action = read("deployment/action.yml");
    assert!(line_then(
        &action,
        &line(2, "agent-model:"),
        &line(4, "value: ${{ steps.read.outputs.agent-model }}")
    ));
}

#[test]
fn dispatch_wrapper_admits_the_callers_owner_before_any_other_job() {
    admits_first(&read(AGENTIC_JOB), "target");
    // The issue's case: an owner of no organization of ours fails that step.
    let action = YamlLoader::load_from_str(&read("admit/action.yml"))
        .unwrap()
        .remove(0);
    let steps = action["runs"]["steps"].as_vec().unwrap();
    let [step] = steps.as_slice() else {
        panic!("admit has {} steps", steps.len());
    };
    assert_eq!(step["shell"].as_str(), Some("bash"));
    let Yaml::Hash(env) = &step["env"] else {
        panic!("the admit step has no env");
    };
    let names: BTreeSet<_> = env.keys().filter_map(Yaml::as_str).collect();
    assert_eq!(names, BTreeSet::from(["ACTION_PATH", "OWNER", "OWNER_ID"]));
    let run = Command::new("bash")
        .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c"])
        .arg(step["run"].as_str().unwrap())
        .env("OWNER", "caller")
        .env("OWNER_ID", "424242")
        .env("ACTION_PATH", root().join("admit"))
        .output()
        .expect("bash");
    assert_eq!(
        run.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

#[test]
fn review_wrapper_admits_the_callers_owner_before_any_other_job() {
    admits_first(&read(REVIEW), "admit");
}

#[test]
fn policy_is_given_skills_only_from_the_target_job() {
    let workflow = read(AGENTIC_JOB);
    assert!(workflow.contains(&line(10, "SKILLS: ${{ inputs.skills }}")));
    assert!(workflow.contains(&line(
        6,
        "agent-skills: ${{ needs.target.outputs.skills == 'true' }}"
    )));
    assert!(workflow.contains(&line(6, "skills: ${{ steps.target.outputs.skills }}")));
}

#[test]
fn policy_is_given_max_requests_only_from_the_target_job() {
    let workflow = read(AGENTIC_JOB);
    assert!(workflow.contains(&line(10, "MAX_REQUESTS: ${{ inputs.max-requests }}")));
    assert!(workflow.contains(&line(
        6,
        "max-requests: ${{ needs.target.outputs.max-requests }}"
    )));
    assert!(workflow.contains(&line(
        6,
        "max-requests: ${{ steps.target.outputs.max-requests }}"
    )));
}

#[test]
fn tailnet_join_is_for_a_real_agent_or_a_session() {
    assert!(job(&read(AGENTIC_JOB), "agent").contains(
        "- name: Join the tailnet\n        if: ${{ needs.target.outputs.agent != 'fake' || needs.target.outputs.session == 'true' }}\n"
    ));
}

#[test]
fn session_job_posts_the_endpoint_before_apply_concludes_and_only_for_a_session() {
    let workflow = read(AGENTIC_JOB);
    let session = job(&workflow, "session");
    assert!(session.contains(&line(4, "needs: [target, policy]")));
    assert!(session.contains(&line(
        4,
        "if: ${{ needs.policy.outputs.admitted == 'true' && needs.target.outputs.session == 'true' && needs.policy.outputs.notified == 'true' && needs.policy.outputs.notify == 'comment' && needs.policy.outputs.binary-artifact-id != '' }}"
    )));
    assert!(session.contains(
        "\n    concurrency:\n      group: agentic-job-status-${{ needs.policy.outputs.notify-repo }}-${{ needs.policy.outputs.notify-item }}\n      cancel-in-progress: false\n"
    ));
    assert!(session.contains(&line(4, "runs-on: ubuntu-26.04")));
    assert!(session.contains(&line(4, "continue-on-error: true")));
    assert!(session.contains("`${{ needs.target.outputs.session-endpoint }}`"));
    assert!(session.contains(
        "\n          artifact-ids: ${{ needs.policy.outputs.binary-artifact-id }}\n          path: ${{ runner.temp }}/bin\n"
    ));
    assert!(session.contains(
        "\n          chmod +x \"$RUNNER_TEMP/bin/agentic-job\"\n          \"$RUNNER_TEMP/bin/agentic-job\" status\n"
    ));
    // Only the policy binary, never an agent-uploaded executable.
    for agents in ["actions/checkout", ".agentic-job-source", "needs.agent"] {
        assert!(!session.contains(agents), "{agents}");
    }
    assert!(job(&workflow, "apply").contains(&line(4, "needs: [policy, agent, check, session]")));
}

/// Appends its arguments to CALL_LOG, each followed by a tab, one call to
/// a line.
const FAKE_SUDO: &str = r#"#!/bin/sh
{ printf '%s\t' "$@"; printf '\n'; } >> "$CALL_LOG"
"#;

// Execute the agent job's first step against stub os-release files with a
// recording sudo double: only RHEL 10 installs or links anything, and it
// does so before the first checkout, which needs git.
#[test]
fn only_rhel_10_installs_git_node_and_gh_and_links_agentic_job_into_sudos_path() {
    use std::os::unix::fs::PermissionsExt;

    let agent = job(&read(AGENTIC_JOB), "agent");
    let step = agent
        .split("      - name: Install RHEL prerequisites\n")
        .nth(1)
        .map(|rest| rest.split("\n      - ").next().unwrap())
        .expect("the agent job has no RHEL prerequisites step");
    assert!(steps_begin_with(
        &agent,
        "- name: Install RHEL prerequisites"
    ));
    assert!(step.contains(&line(10, "OS_RELEASE: /etc/os-release")));
    assert!(!step.contains("\n        if:"));
    let script = step
        .split("        run: |\n")
        .nth(1)
        .unwrap()
        .split('\n')
        .map(|line| line.strip_prefix("          ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let home = std::env::var_os("HOME").expect("HOME");
    let directory = TempDir::new(Path::new(&home), "agent-prerequisites");
    let log = directory.0.join("calls");
    let os_release = directory.0.join("os-release");
    let sudo = directory.0.join("sudo");
    fs::write(&sudo, FAKE_SUDO).unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();
    let rhel: &[&[&str]] = &[
        &["dnf", "install", "-y", "git", "nodejs", "podman"],
        &[
            "dnf",
            "install",
            "-y",
            "https://dl.fedoraproject.org/pub/epel/epel-release-latest-10.noarch.rpm",
        ],
        &["dnf", "install", "-y", "gh"],
        &[
            "ln",
            "-sfn",
            "/usr/local/bin/agentic-job",
            "/usr/bin/agentic-job",
        ],
    ];
    let path = format!(
        "{}:{}",
        directory.0.display(),
        std::env::var("PATH").unwrap()
    );
    for (release, expected) in [
        (
            Some(
                "NAME=\"Red Hat Enterprise Linux\"\nID=\"rhel\"\nID_LIKE=\"centos fedora\"\nVERSION_ID=\"10.0\"\n",
            ),
            rhel,
        ),
        (Some("ID=rhel\nVERSION_ID=10\n"), rhel),
        (
            Some("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"26.04\"\n"),
            &[],
        ),
        (Some("ID=\"rhel\"\nVERSION_ID=\"9.6\"\n"), &[]),
        (Some("ID=\"rhel\"\nVERSION_ID=\"100\"\n"), &[]),
        (
            Some("ID=\"centos\"\nID_LIKE=\"rhel fedora\"\nVERSION_ID=\"10\"\n"),
            &[],
        ),
        (Some(""), &[]),
        (None, &[]),
    ] {
        match release {
            Some(release) => fs::write(&os_release, release).unwrap(),
            None => {
                let _ = fs::remove_file(&os_release);
            }
        }
        fs::write(&log, "").unwrap();
        let result = Command::new("bash")
            .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c"])
            .arg(&script)
            .env("PATH", &path)
            .env("CALL_LOG", &log)
            .env("OS_RELEASE", &os_release)
            .output()
            .expect("bash");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let calls = fs::read_to_string(&log).unwrap();
        let calls: Vec<Vec<&str>> = calls
            .split('\n')
            .filter(|call| !call.is_empty())
            .map(|call| {
                call.strip_suffix('\t')
                    .unwrap_or(call)
                    .split('\t')
                    .collect()
            })
            .collect();
        assert_eq!(calls, expected, "{release:?}");
    }
}
