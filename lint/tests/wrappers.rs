//! The wrapper workflows' structure, read as text from .github/workflows,
//! and the few steps of theirs that are shell, run through bash. The tests
//! that execute the target job's github-script step or Renovate's regular
//! expressions stay in
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
// Both wrappers' agent job, called by local path.
const AGENT: &str = ".github/workflows/cgwalters-agent.yml";

// Keep refused legacy inputs declared so callers get an actionable target
// error, but never forward a provider credential or session listener to the
// runner-only agent. The executable refusal cases remain in the Node suite.
#[test]
fn runner_only_refused_inputs_and_credential_wiring() {
    let text = read(AGENTIC_JOB);
    let workflow = YamlLoader::load_from_str(&text).unwrap().remove(0);
    let call = &workflow["on"]["workflow_call"];
    for (name, kind, default) in [
        ("session", "boolean", Yaml::Boolean(false)),
        ("idle-minutes", "string", Yaml::String(String::new())),
        (
            "inference-register",
            "string",
            Yaml::String("github-oidc".into()),
        ),
        ("inference-provider", "string", Yaml::String(String::new())),
    ] {
        let input = &call["inputs"][name];
        assert_eq!(input["type"].as_str(), Some(kind), "{name}");
        assert_eq!(input["default"], default, "{name}");
        assert!(!input["description"].as_str().unwrap().is_empty(), "{name}");
    }
    assert_eq!(
        call["secrets"]["INFERENCE_API_KEY"]["required"],
        Yaml::Boolean(false)
    );
    // Exactly one reference, reduced to a boolean before entering the script.
    assert_eq!(captures(&text, "secrets.INFERENCE_API_KEY"), [" != '' }}"]);
    let target = &workflow["jobs"]["target"];
    let envs: Vec<_> = target["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .filter_map(|step| step["env"]["INFERENCE_API_KEY_SET"].as_str())
        .collect();
    assert_eq!(envs, ["${{ secrets.INFERENCE_API_KEY != '' }}"]);
    assert_eq!(
        workflow["jobs"]["policy"]["with"]["inference-register"].as_str(),
        Some("github-oidc")
    );
    let policy = job(&text, "policy");
    for forbidden in [
        line(6, "session:"),
        "idle-minutes".into(),
        "inference-provider".into(),
    ] {
        assert!(!policy.contains(&forbidden), "policy: {forbidden}");
    }
    assert!(workflow["jobs"]["policy"]["with"]["session"].is_badvalue());
    assert!(workflow["jobs"]["agent"]["with"]["session"].is_badvalue());
    for forbidden in [
        "TAILNET_DOMAIN",
        "session-host",
        "session-endpoint",
        "inference-api-key",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    let agent = read(AGENT);
    for forbidden in [
        "inputs.session",
        "statedir:",
        "inference-api-key",
        "mcp-token",
        "agent-user",
        "INFERENCE_API_KEY",
    ] {
        assert!(!agent.contains(forbidden), "agent: {forbidden}");
    }
}

#[test]
fn inference_defaults_remain_coupled_in_the_target_script() {
    let text = read(AGENTIC_JOB);
    let workflow = YamlLoader::load_from_str(&text).unwrap().remove(0);
    let steps = workflow["jobs"]["target"]["steps"].as_vec().unwrap();
    let step = steps
        .iter()
        .find(|step| !step["env"]["INFERENCE_URL"].is_badvalue())
        .unwrap();
    // Independent fallbacks here could send our audience to another URL;
    // the target script must choose the URL/audience pair together instead.
    for (name, expected) in [
        (
            "INFERENCE_URL",
            "${{ inputs.inference-url || vars.INFERENCE_URL }}",
        ),
        (
            "INFERENCE_AUDIENCE",
            "${{ inputs.inference-audience || vars.INFERENCE_AUDIENCE }}",
        ),
        ("INFERENCE_REGISTER", "${{ inputs.inference-register }}"),
    ] {
        assert_eq!(step["env"][name].as_str(), Some(expected), "{name}");
    }
}

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

// Marks a commit pin of our own actions as taken from main, for Renovate
// (.github/renovate.json). More than one word, so not a ref to zizmor.
const MAIN_PIN_COMMENT: &str = "# from main";

/// Whether R, what follows a `uses:` line's `@`, is a full commit marked as
/// taken from main.
fn main_pin(r: &str) -> bool {
    r.strip_suffix(MAIN_PIN_COMMENT)
        .and_then(|r| r.strip_suffix(' '))
        .is_some_and(is_commit)
}

// A ref comment of main would hold the pin to main's tip, failing lint at
// every merge after the pin's commit; a pin taken from main need not stay
// its tip. The online test in zizmor.rs checks the real audit, not a model
// of its parser; this offline contract keeps every wrapper on that marker.
#[test]
fn main_pins_use_the_tested_annotation() {
    let commit = "e084c52a9aa07f1279182cfc738de68b96016278";
    assert!(main_pin(&format!("{commit} {MAIN_PIN_COMMENT}")));
    assert!(!main_pin(&format!("{commit} # main")));
    for file in [AGENTIC_JOB, REVIEW, AGENT] {
        let text = read(file);
        for (action, r) in own_action_refs(&text) {
            if is_commit(r) {
                let pin = captures(&text, &format!("uses: jmarrero-forge/actions/{action}@{r}"));
                assert!(
                    pin.iter().all(|c| c.trim() == MAIN_PIN_COMMENT),
                    "{file}: {action}@{r}: {pin:?}"
                );
            }
        }
    }
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

#[test]
fn dispatched_review_keeps_base_policy_and_credential_free_head_access() {
    let workflow = read(AGENTIC_JOB);
    let policy = job(&workflow, "policy");
    for required in [
        "review-head: ${{ inputs.kind == 'review' && inputs.head || '' }}",
        "review-task: workflow/review.md",
        "allow: ${{ inputs.kind == 'review' && needs.target.outputs.review-allow",
        "inputs.kind == 'review' && needs.target.outputs.review-config",
        "budget: ${{ inputs.kind == 'review' && '500'",
        "timeout: '75'",
    ] {
        assert!(policy.contains(required), "policy must retain {required}");
    }
    let agent_job = job(&workflow, "agent");
    for required in [
        line(6, "github-reads: ${{ inputs.kind != 'review' }}"),
        line(4, "uses: ./.github/workflows/cgwalters-agent.yml"),
    ] {
        assert!(
            agent_job.contains(&required),
            "agent must retain {required}"
        );
    }
    let agent = job(&read(AGENT), "agent");
    assert_eq!(agent.matches("uses: actions/checkout@").count(), 2);
    // Both privileged checkouts come from policy, never a PR head or another
    // caller input. Check every ref, not merely the presence of one safe ref.
    assert_eq!(
        agent
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("ref: "))
            .collect::<Vec<_>>(),
        [
            "${{ fromJSON(inputs.policy).ref }}",
            "${{ fromJSON(inputs.policy).source-sha }}",
        ]
    );
    for forbidden in ["git fetch", "pulls/", "agent-user"] {
        assert!(
            !agent.contains(forbidden),
            "agent must not contain {forbidden}"
        );
    }
    assert!(agent.contains("github-token: ${{ inputs.github-reads && github.token || '' }}"));
    assert!(job(&workflow, "apply").contains("inputs.kind != 'review'"));
}

// jmarrero-forge's broker is on a private tailnet address, which this public
// repository never writes: the broker is the calling repository's
// INFERENCE_URL, and the wrappers default only its audience.
#[test]
fn broker_defaults_follow_the_tailnet_configuration() {
    let tailnets: serde_json::Value =
        serde_json::from_str(&read("tailscale/tailnets.json")).unwrap();
    assert_eq!(tailnets["jmarrero-forge"]["proxy"].as_str().unwrap(), "");
    let review = read(REVIEW);
    assert_eq!(
        captures(&review, "\n      inference-url: "),
        ["${{ vars.INFERENCE_URL }}"; 2]
    );
    assert_eq!(
        captures(&review, "\n      inference-audience: "),
        ["${{ vars.INFERENCE_AUDIENCE || 'jmarrero-praxis-broker' }}"; 2]
    );
    let dispatch = read(AGENTIC_JOB);
    assert_eq!(
        captures(&dispatch, "\n          BROKER_URL: "),
        ["${{ vars.INFERENCE_URL }}"]
    );
    assert_eq!(
        captures(&dispatch, "\n          BROKER_AUDIENCE: "),
        ["jmarrero-praxis-broker"]
    );
    for text in [&review, &dispatch] {
        assert!(!text.contains("http://1"), "no tailnet address is written");
    }
}

#[test]
fn review_stays_on_the_inference_broker() {
    let review = read(REVIEW);
    for forbidden in [
        "INFERENCE_API_KEY",
        "inference-register",
        "inference-provider",
        "inference-api-key",
    ] {
        assert!(!review.contains(forbidden), "unexpected {forbidden}");
    }
}

#[test]
fn artifact_only_skips_proposals_and_notifications() {
    let workflow = read(AGENTIC_JOB);
    for (name, required) in [
        (
            "apply",
            "always() && inputs.kind != 'review' && !inputs.research-artifact-only",
        ),
        (
            "policy",
            "SAFE_OUTPUTS_PAT: ${{ inputs.kind != 'review' && !inputs.research-artifact-only && secrets.SAFE_OUTPUTS_PAT || '' }}",
        ),
        (
            "policy",
            "notify: ${{ (inputs.kind == 'review' || inputs.research-artifact-only) && 'none' || 'comment' }}",
        ),
        (
            "policy",
            "apply-environment: ${{ inputs.kind != 'review' && !inputs.research-artifact-only && (inputs.apply-environment || vars.APPLY_ENVIRONMENT) || '' }}",
        ),
        (
            "policy",
            "apply-partial: ${{ !inputs.research-artifact-only && (",
        ),
        (
            "check",
            "safe-outputs-artifact-id: ${{ needs.agent.outputs.safe-outputs-artifact-id }}",
        ),
        (
            "check",
            "safe-outputs-artifact-digest: ${{ needs.agent.outputs.safe-outputs-artifact-digest }}",
        ),
        (
            "research-handback",
            "!cancelled() && inputs.research-artifact-only && inputs.kind == 'research'",
        ),
        ("research-handback", "permissions: {}"),
    ] {
        assert!(
            job(&workflow, name).contains(required),
            "{name}: missing {required}"
        );
    }
    let handback = job(&workflow, "research-handback");
    for forbidden in [
        "secrets.",
        "github.token",
        "download-artifact",
        "checkout",
        "run:",
    ] {
        assert!(
            !handback.contains(forbidden),
            "handback contains {forbidden}"
        );
    }
    let parsed = YamlLoader::load_from_str(&workflow).unwrap();
    for (output, field) in [
        ("research-status", "status"),
        ("safe-outputs-artifact-id", "artifact-id"),
        ("safe-outputs-artifact-digest", "artifact-digest"),
        ("handback-repository", "repository"),
        ("handback-run-id", "run-id"),
        ("handback-run-attempt", "run-attempt"),
        ("handback-agent-job", "agent-job"),
    ] {
        assert_eq!(
            parsed[0]["on"]["workflow_call"]["outputs"][output]["value"].as_str(),
            Some(format!("${{{{ jobs.research-handback.outputs.{field} }}}}").as_str()),
            "{output}",
        );
    }
    let review = read(REVIEW);
    for forbidden in ["research-artifact-only", "research-handback"] {
        assert!(!review.contains(forbidden), "review contains {forbidden}");
    }
}

#[test]
fn checked_submit_preserves_credentials_and_provenance() {
    let workflow = read(AGENTIC_JOB);
    let submit = job(&workflow, "submit");
    for forbidden in ["SAFE_OUTPUTS_PAT", "checkout", "uses: ./"] {
        assert!(!submit.contains(forbidden), "submit contains {forbidden}");
    }
    for required in [
        "GH_TOKEN: ${{ github.token }}",
        "artifact-ids: ${{ needs.policy.outputs.binary-artifact-id }}",
        "artifact-ids: ${{ needs.check.outputs.artifact-id }}",
        "HEAD_SHA: ${{ needs.policy.outputs.review-head }}",
        "AGENT_JOB: ${{ needs.agent.outputs.job }}",
        "review submit --accepted \"$RUNNER_TEMP/checked/agent_output.json\"",
        "!cancelled()",
        "vars.AGENT_REAL_REVIEWS == 'true'",
        "needs.check.result == 'success'",
        "needs.check.outputs.artifact-id != ''",
        "needs.agent.result == 'success'",
        "needs.agent.outputs.exit == '0'",
        "contains(fromJSON('[\"APPROVE\", \"CHANGES\"]'), needs.check.outputs.verdict)",
    ] {
        assert!(submit.contains(required), "submit missing {required}");
    }
    let agent = read(AGENT);
    assert!(agent.contains(&line(6, "job: ${{ job.check_run_id }}")));
    assert!(agent.contains("\n      job:\n        value: ${{ jobs.agent.outputs.job }}\n"));
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
// reviewed wrapper pin. Check every own-repository call, not just tailscale,
// and the tailnet join of each wrapper's register job and of the agent job
// both call, which is privileged for every kind.
#[test]
fn review_wrapper_pins_its_own_actions_by_full_commit() {
    for (file, actions) in [
        (
            REVIEW,
            &["admit", "deployment", "tailscale", "tailscale"][..],
        ),
        // secure-host carries no identity and stays at upstream's commit
        // pin, the only owner workflows-check accepts for it.
        (AGENT, &["tailscale"]),
    ] {
        let text = read(file);
        let refs = own_action_refs(&text);
        assert_eq!(
            refs.iter().map(|r| r.0).collect::<Vec<_>>(),
            actions,
            "{file}"
        );
        for (action, r) in refs {
            assert!(is_commit(r), "{file}: {action}: {r}");
        }
        for r in captures(&text, "uses: jmarrero-forge/actions/tailscale@") {
            assert!(main_pin(r), "{file}: {r}");
        }
    }
}

// Unlike upstream's, our dispatch wrapper pins its own actions for every
// kind: our broker admits it at the commit its caller pins.
#[test]
fn dispatch_wrapper_pins_its_own_actions_for_every_kind() {
    let workflow = read(AGENTIC_JOB);
    let refs = own_action_refs(&workflow);
    assert_eq!(refs.len(), 5);
    for (action, r) in refs {
        assert!(is_commit(r), "{action}: {r}");
    }
    for r in captures(&workflow, "uses: jmarrero-forge/actions/") {
        let r = r.split_once('@').unwrap().1;
        assert!(main_pin(r), "{r}");
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
    const CHECKOUT: &str = "repository: ${{ fromJSON(inputs.policy).source-repository }}";
    // The repository both wrappers' policy `uses:` names.
    let mut called = BTreeSet::new();
    for file in [AGENTIC_JOB, REVIEW] {
        let workflow = read(file);
        let repo = workflow
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
                .then_some(repo.to_owned())
            })
            .unwrap_or_else(|| panic!("{file}: policy call"));
        called.insert(repo);
        // The wrappers check out nothing of the source themselves.
        assert!(!workflow.contains(CHECKOUT), "{file}");
    }
    let [called] = Vec::from_iter(called).try_into().unwrap();
    // The agent job checks out the source; each register job runs the
    // policy call's binary and checks the same first.
    for (file, part, checks_out) in [
        (AGENT, "agent", true),
        (AGENTIC_JOB, "register", false),
        (REVIEW, "register", false),
    ] {
        let workflow = job(&read(file), part);
        let checkout_text = if checks_out {
            CHECKOUT
        } else {
            "repository: ${{ needs.policy.outputs.source-repository }}"
        };
        let steps: Vec<_> = workflow.split("\n      - ").collect();
        let checkout = steps.iter().position(|step| step.contains(checkout_text));
        assert_eq!(checkout.is_some(), checks_out, "{file}");
        let guard = steps[..checkout.unwrap_or(steps.len())]
            .iter()
            .find(|step| step.starts_with("name: The policy call's source"))
            .unwrap_or_else(|| panic!("{file}: checks the source first"));
        let source = if checks_out {
            "fromJSON(inputs.policy)"
        } else {
            "needs.policy.outputs"
        };
        for env in [
            format!("SOURCE_REPOSITORY: ${{{{ {source}.source-repository }}}}"),
            format!("SOURCE_SHA: ${{{{ {source}.source-sha }}}}"),
        ] {
            assert!(guard.contains(&line(10, &env)), "{file}: {env}");
        }
        let expected = captures(guard, "\n          EXPECTED_REPOSITORY: ")
            .into_iter()
            .next()
            .filter(|v| !v.is_empty() && !v.contains(char::is_whitespace));
        assert_eq!(expected, Some(called.as_str()), "{file}");
        assert!(
            guard.contains(
                "[[ \"$SOURCE_REPOSITORY\" == \"$EXPECTED_REPOSITORY\" && \"$SOURCE_SHA\" =~ ^[0-9a-f]{40}$ ]] ||\n            { echo \"::error::the policy call is not $EXPECTED_REPOSITORY at a full commit\"; exit 1; }"
            ),
            "{file}"
        );
    }
}

#[test]
fn composed_review_links_its_pinned_helper_to_the_agent_job() {
    let workflow = read(REVIEW);
    let shared = read(AGENT);
    assert!(shared.contains("\n      job:\n        value: ${{ jobs.agent.outputs.job }}\n"));
    let agent = job(&shared, "agent");
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

#[test]
fn review_task_requires_project_visibility_before_claiming_missing_membership() {
    let task = read("workflow/review.md");
    let task = task.split_whitespace().collect::<Vec<_>>().join(" ");
    for contract in [
        "An empty `gh issue view --json projectItems` result is not proof",
        "same token can read the exact target Project",
        "complete membership query",
        "membership could not be verified",
        "Do not turn that uncertainty into a missing-membership finding or blocker",
        "Do not request broader credentials",
    ] {
        assert!(task.contains(contract), "{contract}");
    }
}

// agentic-job is called at one release: check, apply, prepare and run
// refuse calls that resolved to different commits, which following main
// produced for runs spanning an agentic-job merge (tracker#544).
// Our own admit and deployment actions still follow main for every kind but
// review.
#[test]
fn agentic_job_is_called_at_one_release_our_own_actions_at_main() {
    let workflow = read(AGENTIC_JOB);
    // Ours calls agentic-job from cgwalters-forge and its own actions here.
    let refs: Vec<_> = captures(&workflow, "uses: ")
        .into_iter()
        .filter_map(|rest| {
            rest.strip_prefix("cgwalters-forge/agentic-job/")
                .map(|rest| ("agentic-job/", rest))
                .or_else(|| {
                    rest.strip_prefix("jmarrero-forge/actions/")
                        .map(|rest| ("actions/", rest))
                })
        })
        .filter_map(|(repo, rest)| {
            let (token, comment) = rest
                .split_once(' ')
                .map_or((rest, None), |(t, c)| (t, Some(c)));
            if let Some(comment) = comment.filter(|c| *c != MAIN_PIN_COMMENT) {
                let tag = comment.strip_prefix("# ")?;
                if tag.is_empty() || tag.contains(char::is_whitespace) {
                    return None;
                }
            }
            token
                .rsplit_once('@')
                .filter(|(path, r)| !path.is_empty() && !r.is_empty())
                .map(|(path, r)| (format!("{repo}{path}"), r))
        })
        .collect();
    assert_eq!(
        refs.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "actions/admit",
            "actions/admit",
            "actions/deployment",
            "actions/deployment",
            "agentic-job/.github/workflows/policy.yml",
            "actions/tailscale",
            "agentic-job/.github/workflows/check.yml",
            "agentic-job/.github/workflows/apply.yml",
        ]
    );
    for (path, r) in &refs {
        if path.starts_with("actions/") {
            assert!(*r == "main" || is_commit(r), "{path}");
        }
    }
    assert!(!workflow.contains("PINNED_SHA"));
    for file in [AGENT, AGENTIC_JOB, REVIEW] {
        assert!(
            read(file).contains(&line(
                10,
                "EXPECTED_REPOSITORY: cgwalters-forge/agentic-job"
            )),
            "{file}"
        );
    }
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
    // Policy carries the optional item repository through status, check and
    // apply; cross-repository PR references are Refs rather than Closes.
    let policy = job(&workflow, "policy");
    for input in [
        "item-repo: ${{ inputs.item-repo }}",
        "id: ${{ inputs.item-repo || inputs.repo }}#${{ inputs.item }}",
        "repo: ${{ inputs.repo }}",
        "output-repo: ${{ inputs.repo }}",
        "comment-target: ${{ inputs.item }}",
    ] {
        assert!(policy.contains(&line(6, input)), "{input}");
    }
}

#[test]
fn dispatch_item_repository_defaults_and_routes_issue_text_without_moving_work() {
    let workflow = YamlLoader::load_from_str(&read(AGENTIC_JOB))
        .unwrap()
        .remove(0);
    let input = &workflow["on"]["workflow_call"]["inputs"]["item-repo"];
    assert_eq!(input["type"].as_str(), Some("string"));
    assert_eq!(input["default"].as_str(), Some(""));
    assert_ne!(input["required"].as_bool(), Some(true));
    let step = workflow["jobs"]["target"]["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .find(|step| step["id"].as_str() == Some("target"))
        .unwrap();
    assert_eq!(
        step["env"]["ITEM_REPO"].as_str(),
        Some("${{ inputs.item-repo }}")
    );
    assert_eq!(
        step["env"]["TARGET_REPO"].as_str(),
        Some("${{ inputs.repo }}")
    );
    let harness = r#"
const assert = require('node:assert/strict');
let task;
let calls = 0;
const github = { rest: {
  repos: { get: async () => ({ data: { private: false, visibility: 'public' } }) },
  issues: { get: async args => {
    calls++;
    assert.deepEqual(args, { owner: 'forge', repo: process.env.ITEM_REPO ? 'tracker' : 'work', issue_number: 42 });
    return { data: { number: 42, title: 'Staged issue', body: 'Task body', user: { login: 'operator' }, author_association: 'OWNER' } };
  } }
} };
const localRequire = name => name === 'node:fs/promises'
  ? { writeFile: async (_path, text) => { task = text; } } : require(name);
const run = new (Object.getPrototypeOf(async function(){}).constructor)('github', 'context', 'core', 'require', process.argv[1]);
run(github, { repo: { owner: 'forge', repo: 'caller' } }, { setOutput() {} }, localRequire).then(() => {
  assert.equal(process.env.EXPECT_ERROR, '');
  assert.equal(calls, 1);
  const event = JSON.parse(task.split('<agentic-job-event>\n')[1].split('\n</agentic-job-event>')[0]);
  assert.equal(event.repository, process.env.ITEM_REPO || 'forge/work');
  assert.equal(event.number, 42);
}).catch(error => {
  if (!process.env.EXPECT_ERROR) throw error;
  assert.match(error.message, new RegExp(process.env.EXPECT_ERROR));
  assert.equal(calls, 0);
});
"#;
    for (item_repo, profile, error) in [
        ("", "implement", ""),
        ("forge/tracker", "implement", ""),
        ("forge/tracker", "triage", ""),
        ("forge/tracker", "research", ""),
        ("forge/tracker", "session", ""),
        (
            "forge/tracker",
            "review",
            "item-repo is not for review or fix",
        ),
        ("forge/tracker", "fix", "item-repo is not for review or fix"),
        (" forge/tracker", "implement", "Expected OWNER/NAME"),
        ("forge/tracker/extra", "implement", "Expected OWNER/NAME"),
    ] {
        let output = Command::new("node")
            .args(["-e", harness, step["with"]["script"].as_str().unwrap()])
            .envs([
                ("TARGET_REPO", "forge/work"),
                ("TARGET_ITEM", "42"),
                ("ITEM_REPO", item_repo),
                ("PROFILE", profile),
                ("EXPECT_ERROR", error),
                ("SESSION", "false"),
                ("IDLE_MINUTES", ""),
                ("INFERENCE_REGISTER", "github-oidc"),
                ("INFERENCE_PROVIDER", ""),
                ("INFERENCE_API_KEY_SET", "false"),
                ("AGENT", "opencode"),
                ("AGENT_RUNNER", "\"ubuntu-26.04\""),
                ("APPLY_ENVIRONMENT", "agent-apply"),
                ("MAX_REQUESTS", "400"),
                ("INFERENCE_URL", ""),
                ("INFERENCE_AUDIENCE", ""),
                ("AGENT_CONFIG_REPO", "forge/config"),
                ("RUNNER_TEMP", "/tmp/opencode"),
            ])
            .output()
            .expect("node");
        assert!(
            output.status.success(),
            "{item_repo:?}/{profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
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
    // Pinned by commit, unlike upstream's @main (see the wrapper).
    let target = job(&workflow, "target");
    let pin = captures(
        &target,
        "\n      - uses: jmarrero-forge/actions/deployment@",
    );
    assert!(pin.iter().all(|r| main_pin(r)), "{pin:?}");
    assert!(target.contains(&format!(
        "\n      - uses: jmarrero-forge/actions/deployment@{}\n        id: deployment\n",
        pin[0]
    )));
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
fn safe_output_contract_keeps_caller_bounds_counts_and_apply_token() {
    let dispatch = read(AGENTIC_JOB);
    let policy = job(&dispatch, "policy");
    assert!(policy.contains(&line(
        6,
        "allow: ${{ inputs.kind == 'review' && needs.target.outputs.review-allow || (inputs.kind == 'fix' && inputs.fix-allow || inputs.allow) }}"
    )));
    assert!(policy.contains(&line(
        6,
        "max-outputs: ${{ inputs.kind == 'review' && '1' || '3' }}"
    )));
    let review = read(REVIEW);
    let review_policy = job(&review, "policy");
    assert!(review_policy.contains(&line(6, "max-outputs: '1'")));
    assert!(review_policy.contains(&line(
        6,
        "allow: ${{ github.event_name == 'issue_comment' && 'workflow/review-command.toml' || 'workflow/review.toml' }}"
    )));
    for workflow in [&dispatch, &review] {
        let apply = job(workflow, "apply");
        assert!(apply.contains(&line(
            6,
            "SAFE_OUTPUTS_PAT: ${{ secrets.SAFE_OUTPUTS_PAT }}"
        )));
        assert!(apply.contains(&line(6, "policy: ${{ toJSON(needs.policy.outputs) }}")));
        assert!(apply.contains(&line(6, "check: ${{ toJSON(needs.check) }}")));
        for name in ["register", "agent", "check"] {
            assert!(!job(workflow, name).contains("SAFE_OUTPUTS_PAT"), "{name}");
        }
    }
}

// The agent job has no identity token: it joins with the key the register
// job minted, as root before anything of the agent's, for every kind.
#[test]
fn agent_joins_the_tailnet_with_the_register_jobs_key() {
    let agent = job(&read(AGENT), "agent");
    assert!(agent.contains(
        "\n      - name: Join the tailnet\n        uses: jmarrero-forge/actions/tailscale@"
    ));
    assert!(agent.contains(&line(10, "authkey: ${{ secrets.TAILNET_KEY }}")));
    for file in [AGENTIC_JOB, REVIEW] {
        let workflow = read(file);
        let register = job(&workflow, "register");
        assert!(register.contains(&line(10, "mint: 'true'")), "{file}");
        assert!(
            register.contains(&line(6, "tailnet-key: ${{ steps.tailnet.outputs.key }}")),
            "{file}"
        );
        assert_eq!(
            workflow.matches("- name: Join the tailnet").count(),
            if file == REVIEW { 2 } else { 1 },
            "{file}"
        );
        assert!(
            job(&workflow, "agent").contains(&line(
                6,
                "TAILNET_KEY: ${{ needs.register.outputs.tailnet-key }}"
            )),
            "{file}"
        );
    }
}

// The key a re-run of only the agent job reuses, or one that expired while
// the job waited, is refused before the join with what to do. Both
// wrappers pass the register job's attempt and mint time; the step order
// and the register job's outputs are review_wrapper.rs's.
#[test]
fn agent_refuses_a_spent_or_expired_tailnet_key_before_the_join() {
    let agent = job(&read(AGENT), "agent");
    let step = agent
        .split("      - name: Check the tailnet key is this attempt's and unexpired\n")
        .nth(1)
        .map(|rest| rest.split("\n      - ").next().unwrap())
        .expect("the agent job has no tailnet key check");
    let mint = read("tailscale/mint.mjs");
    assert_eq!(
        captures(&mint, "export const KEY_EXPIRY_SECONDS = "),
        ["3600;"]
    );
    assert!(step.contains(&line(10, "KEY_EXPIRY_SECONDS: 3600")));
    for file in [AGENTIC_JOB, REVIEW] {
        let call = job(&read(file), "agent");
        for input in ["tailnet-key-attempt", "tailnet-key-minted-at"] {
            assert!(
                call.contains(&line(
                    6,
                    &format!("{input}: ${{{{ needs.register.outputs.{input} }}}}")
                )),
                "{file}: {input}"
            );
        }
    }
    let script = step.split("        run: |\n").nth(1).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let fresh = (now - 60).to_string();
    let stale = (now - 3600).to_string();
    // (minted in attempt, this attempt, mint time, error)
    for (minted, attempt, minted_at, want) in [
        ("2", "2", fresh.as_str(), None),
        ("1", "1", stale.as_str(), Some("expired")),
        ("1", "2", fresh.as_str(), Some("attempt 1's")),
        ("1", "1", "", Some("no mint time")),
    ] {
        let run = Command::new("bash")
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                script,
            ])
            .env("MINTED_ATTEMPT", minted)
            .env("GITHUB_RUN_ATTEMPT", attempt)
            .env("MINTED_AT", minted_at)
            .env("KEY_EXPIRY_SECONDS", "3600")
            .output()
            .expect("bash");
        let stdout = String::from_utf8_lossy(&run.stdout);
        match want {
            None => assert!(run.status.success(), "{stdout}"),
            Some(error) => {
                assert_eq!(run.status.code(), Some(1), "{stdout}");
                assert!(stdout.contains(error), "{stdout}");
            }
        }
    }
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
    let agent = job(&read(AGENT), "agent");
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
    check_rhel_prerequisites(&script, rhel);
}

#[test]
fn only_rhel_10_installs_iptables_and_links_tailscale_into_sudos_path() {
    let action = YamlLoader::load_from_str(&read("tailscale/action.yml"))
        .unwrap()
        .remove(0);
    let steps = action["runs"]["steps"].as_vec().unwrap();
    let index = |name| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("missing step {name}"))
    };
    let prerequisites = index("Install RHEL prerequisites");
    assert!(prerequisites < index("Connect to the tailnet"));
    let step = &steps[prerequisites];
    assert_eq!(step["env"]["OS_RELEASE"].as_str(), Some("/etc/os-release"));
    assert_eq!(step["shell"].as_str(), Some("bash"));
    assert!(
        step["if"].is_badvalue(),
        "prerequisites must run on every OS"
    );
    check_rhel_prerequisites(
        step["run"].as_str().expect("prerequisites run script"),
        &[
            &["dnf", "install", "-y", "iptables"],
            &[
                "ln",
                "-sfn",
                "/usr/local/bin/tailscale",
                "/usr/bin/tailscale",
            ],
            &[
                "ln",
                "-sfn",
                "/usr/local/bin/tailscaled",
                "/usr/bin/tailscaled",
            ],
        ],
    );
}

#[test]
fn tailscale_mint_validates_before_credentials_and_uses_the_join_settings() {
    let action = YamlLoader::load_from_str(&read("tailscale/action.yml"))
        .unwrap()
        .remove(0);
    let steps = action["runs"]["steps"].as_vec().unwrap();
    let validate = &steps[0];
    assert_eq!(validate["name"].as_str(), Some("Check the login inputs"));
    assert_eq!(validate["shell"].as_str(), Some("bash"));
    assert!(validate["if"].is_badvalue());
    for (key, input) in [("AUTHKEY", "authkey"), ("MINT", "mint")] {
        assert_eq!(
            validate["env"][key].as_str().unwrap(),
            format!("${{{{ inputs.{input} }}}}")
        );
    }
    for authkey in ["", "fixture-key"] {
        for mint in ["false", "true", "yes", "", "TRUE", " true", "true\n"] {
            let expected_error = match mint {
                "true" if !authkey.is_empty() => {
                    Some("mint needs the federated login, not authkey")
                }
                "true" | "false" => None,
                _ => Some("mint must be true or false"),
            };
            let output = Command::new("bash")
                .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c"])
                .arg(validate["run"].as_str().unwrap())
                .env_clear()
                .env("AUTHKEY", authkey)
                .env("MINT", mint)
                .output()
                .expect("run login validation with bash");
            assert_eq!(
                output.status.code(),
                Some(i32::from(expected_error.is_some())),
                "authkey={authkey:?}, mint={mint:?}: {output:?}"
            );
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                expected_error.map_or_else(String::new, |e| format!("::error::{e}\n"))
            );
            assert!(output.stderr.is_empty());
        }
    }

    let index = |name| {
        steps
            .iter()
            .position(|step| step["name"].as_str() == Some(name))
            .unwrap()
    };
    let settings = index("Choose organization settings");
    let join = index("Connect to the tailnet");
    let harden = index("Make tailscaled's LocalAPI root's only");
    let mint = index("Mint a key for a later job");
    assert!(0 < settings && settings < join && join < harden && harden < mint);
    assert_eq!(action["inputs"]["mint"]["default"].as_str(), Some("false"));
    assert_eq!(steps[mint]["if"].as_str(), Some("inputs.mint == 'true'"));
    assert_eq!(steps[mint]["id"].as_str(), Some("mint"));
    assert_eq!(
        action["outputs"]["key"]["value"].as_str(),
        Some("${{ steps.mint.outputs.key }}")
    );
    assert_eq!(
        steps[join]["with"]["authkey"].as_str(),
        Some("${{ inputs.authkey }}")
    );
    for (key, input) in [
        ("OAUTH_CLIENT_ID", "oauth-client-id"),
        ("AUDIENCE", "audience"),
        ("TAGS", "tags"),
    ] {
        assert_eq!(
            steps[mint]["env"][key].as_str().unwrap(),
            format!("${{{{ steps.settings.outputs.{input} }}}}")
        );
        assert_eq!(
            steps[join]["with"][input].as_str().unwrap(),
            format!("${{{{ inputs.authkey == '' && steps.settings.outputs.{input} || '' }}}}")
        );
    }
}

#[test]
fn tailscale_hardening_runs_after_successful_and_failed_joins() {
    use std::os::unix::fs::PermissionsExt;

    let action = YamlLoader::load_from_str(&read("tailscale/action.yml"))
        .unwrap()
        .remove(0);
    let steps = action["runs"]["steps"].as_vec().unwrap();
    let index = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Make tailscaled's LocalAPI root's only"))
        .expect("LocalAPI hardening step");
    assert_eq!(steps[index - 1]["id"].as_str(), Some("join"));
    let step = &steps[index];
    assert_eq!(step["if"].as_str(), Some("always()"));
    assert_eq!(step["shell"].as_str(), Some("bash"));
    assert_eq!(
        step["env"]["JOINED"].as_str(),
        Some("${{ steps.join.outcome == 'success' }}")
    );

    // Record exact argv as JSON; never invoke sudo or change host permissions.
    const SUDO: &str = r#"#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ['CALL_LOG'], 'a') as log: log.write(json.dumps(args) + '\n')
if args[0] == 'test': sys.exit(0 if os.environ['CLI_EXISTS' if args[1] == '-f' else 'DIRECTORY_EXISTS'] == 'true' else 1)
if args[0] == 'chmod' and args[2] == os.environ['FAIL_CHMOD']: sys.exit(1)
if args[0] == 'stat': print('700 root:root')
"#;
    let home = std::env::var_os("HOME").expect("HOME");
    let directory = TempDir::new(Path::new(&home), "tailscale-hardening");
    let sudo = directory.0.join("sudo");
    fs::write(&sudo, SUDO).unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();
    let log = directory.0.join("calls");
    let path = std::env::join_paths(
        std::iter::once(directory.0.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    const CLI: &str = "/usr/local/bin/tailscale";
    const SOCKET_DIR: &str = "/var/run/tailscale";
    for joined in [false, true] {
        for (cli, socket_dir, fail_chmod) in [
            (true, true, ""),
            (false, false, ""),
            (true, false, ""),
            (false, true, ""),
            (true, true, CLI),
            (true, true, SOCKET_DIR),
        ] {
            fs::write(&log, "").unwrap();
            let result = Command::new("bash")
                .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c"])
                .arg(step["run"].as_str().expect("hardening script"))
                .env("PATH", &path)
                .env("CALL_LOG", &log)
                .env("JOINED", joined.to_string())
                .env("CLI_EXISTS", cli.to_string())
                .env("DIRECTORY_EXISTS", socket_dir.to_string())
                .env("FAIL_CHMOD", fail_chmod)
                .output()
                .expect("bash");
            let scenario =
                format!("joined={joined}, cli={cli}, directory={socket_dir}, fail={fail_chmod}");
            assert_eq!(
                result.status.code(),
                Some(if fail_chmod.is_empty() { 0 } else { 1 }),
                "{scenario}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let mut expected = vec![vec!["test", "-f", CLI]];
            if cli {
                expected.push(vec!["chmod", "0700", CLI]);
            }
            if fail_chmod != CLI {
                expected.push(vec!["test", "-d", SOCKET_DIR]);
                if socket_dir {
                    expected.push(vec!["chmod", "0700", SOCKET_DIR]);
                    if fail_chmod != SOCKET_DIR {
                        expected.push(vec!["stat", "-c", "%a %U:%G", SOCKET_DIR]);
                        if joined {
                            expected.push(vec![CLI, "status", "--self"]);
                        }
                    }
                }
            }
            let calls: Vec<Vec<String>> = fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).expect("sudo argv record"))
                .collect();
            assert_eq!(calls, expected, "{scenario}");
        }
    }
}

// Run the actual shell from each caller, not a Rust model of its OS check.
// The sudo double records ordered argv without installing or linking anything.
fn check_rhel_prerequisites(script: &str, rhel: &[&[&str]]) {
    use std::os::unix::fs::PermissionsExt;

    let home = std::env::var_os("HOME").expect("HOME");
    let directory = TempDir::new(Path::new(&home), "rhel-prerequisites");
    let log = directory.0.join("calls");
    let os_release = directory.0.join("os-release");
    let sudo = directory.0.join("sudo");
    fs::write(&sudo, FAKE_SUDO).unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o700)).unwrap();
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
        (Some("ID=rhel\nVERSION_ID=10.1\n"), rhel),
        (Some("NAME=Ubuntu\nID=ubuntu\nVERSION_ID=24.04\n"), &[]),
        (
            Some("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\nVERSION_ID=\"26.04\"\n"),
            &[],
        ),
        (Some("ID=\"rhel\"\nVERSION_ID=\"9.6\"\n"), &[]),
        (Some("ID=\"rhel\"\nVERSION_ID=\"100\"\n"), &[]),
        (Some("ID=rhel\n"), &[]),
        (Some("VERSION_ID=10\n"), &[]),
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
            .arg(script)
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
