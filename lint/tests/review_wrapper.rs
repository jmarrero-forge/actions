//! Review admission and publication boundaries; no live forge or tailnet calls.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use yaml_rust2::{Yaml, YamlLoader};

// The release both wrappers call, by immutable commit with its tag as a
// comment. A bump changes this, the six calls, the header and the README.
const PIN: &str = "57119eeffa17ea41b383cfdaf8370b8abeba8868 # v0.6.9";
const REVIEW: &str = ".github/workflows/cgwalters-review.yml";
const DISPATCH: &str = ".github/workflows/cgwalters-agentic-job.yml";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(file: &str) -> String {
    fs::read_to_string(root().join(file)).unwrap()
}

fn yaml(text: &str) -> Yaml {
    YamlLoader::load_from_str(text).unwrap().remove(0)
}

fn job<'a>(source: &'a str, name: &str) -> &'a str {
    let rest = source.split_once(&format!("\n  {name}:\n")).unwrap().1;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.starts_with("  ")
            && !line.starts_with("   ")
            && line.trim_end().ends_with(':')
            && !line.trim_start().starts_with('#')
        {
            return &rest[..offset];
        }
        offset += line.len();
    }
    rest
}

fn includes(text: &str, expected: &[&str]) {
    for part in expected {
        assert!(text.contains(part), "missing {part:?} in {text}");
    }
}

fn excludes(text: &str, forbidden: &[&str]) {
    for part in forbidden {
        assert!(!text.contains(part), "unexpected {part:?} in {text}");
    }
}

// A deliberately small Actions-expression evaluator: parse the whole expression
// but evaluate && and || lazily, as Actions does (notably before fromJSON).
#[derive(Debug)]
enum Expr {
    Literal(Yaml),
    Path(String),
    Not(Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

struct Parser<'a>(&'a str);

impl<'a> Parser<'a> {
    fn take(&mut self, token: &str) -> bool {
        self.0 = self.0.trim_start();
        if let Some(rest) = self.0.strip_prefix(token) {
            self.0 = rest;
            true
        } else {
            false
        }
    }

    fn expression(&mut self, level: usize) -> Expr {
        let operators: &[&'static str] = match level {
            0 => &["||"],
            1 => &["&&"],
            2 => &["==", "!="],
            _ => return self.primary(),
        };
        let mut left = self.expression(level + 1);
        loop {
            let Some(op) = operators.iter().find(|op| self.take(op)) else {
                break;
            };
            left = Expr::Binary(op, Box::new(left), Box::new(self.expression(level + 1)));
        }
        left
    }

    fn primary(&mut self) -> Expr {
        if self.take("!") {
            return Expr::Not(Box::new(self.primary()));
        }
        if self.take("(") {
            let expr = self.expression(0);
            assert!(self.take(")"));
            return expr;
        }
        if self.take("'") {
            let mut value = String::new();
            loop {
                let end = self.0.find('\'').expect("unterminated literal");
                value.push_str(&self.0[..end]);
                self.0 = &self.0[end + 1..];
                if let Some(rest) = self.0.strip_prefix('\'') {
                    value.push('\'');
                    self.0 = rest;
                } else {
                    break;
                }
            }
            return Expr::Literal(Yaml::String(value));
        }
        self.0 = self.0.trim_start();
        let end = self
            .0
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
            .unwrap_or(self.0.len());
        assert!(end > 0, "unexpected expression: {}", self.0);
        let name = self.0[..end].to_owned();
        self.0 = &self.0[end..];
        if self.take("(") {
            let mut args = Vec::new();
            if !self.take(")") {
                loop {
                    args.push(self.expression(0));
                    if self.take(")") {
                        break;
                    }
                    assert!(self.take(","));
                }
            }
            Expr::Call(name, args)
        } else {
            match name.as_str() {
                "true" => Expr::Literal(Yaml::Boolean(true)),
                "false" => Expr::Literal(Yaml::Boolean(false)),
                "null" => Expr::Literal(Yaml::Null),
                _ if name.bytes().all(|b| b.is_ascii_digit()) => {
                    Expr::Literal(Yaml::Integer(name.parse().unwrap()))
                }
                _ => Expr::Path(name),
            }
        }
    }
}

fn truthy(value: &Yaml) -> bool {
    match value {
        Yaml::Null | Yaml::BadValue => false,
        Yaml::Boolean(b) => *b,
        Yaml::Integer(n) => *n != 0,
        Yaml::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn string(value: &Yaml) -> String {
    match value {
        Yaml::String(s) => s.to_owned(),
        Yaml::Integer(n) => n.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Null | Yaml::BadValue => String::new(),
        _ => panic!("cannot cast {value:?} to string"),
    }
}

impl Expr {
    fn eval(&self, context: &BTreeMap<&str, Yaml>) -> Yaml {
        match self {
            Self::Literal(v) => v.clone(),
            Self::Path(path) => context.get(path.as_str()).cloned().unwrap_or(Yaml::Null),
            Self::Not(expr) => Yaml::Boolean(!truthy(&expr.eval(context))),
            Self::Binary(op, left, right) => {
                let left = left.eval(context);
                match *op {
                    "&&" if !truthy(&left) => left,
                    "||" if truthy(&left) => left,
                    "&&" | "||" => right.eval(context),
                    "==" | "!=" => {
                        let right = right.eval(context);
                        let equal = match (&left, &right) {
                            (Yaml::String(a), Yaml::String(b)) => a.eq_ignore_ascii_case(b),
                            _ => left == right,
                        };
                        Yaml::Boolean(equal == (*op == "=="))
                    }
                    _ => unreachable!(),
                }
            }
            Self::Call(name, args) => {
                let args: Vec<_> = args.iter().map(|arg| arg.eval(context)).collect();
                match (name.as_str(), args.as_slice()) {
                    ("fromJSON", [value]) => yaml(&string(value)),
                    ("startsWith", [value, prefix]) => Yaml::Boolean(
                        string(value)
                            .to_ascii_lowercase()
                            .starts_with(&string(prefix).to_ascii_lowercase()),
                    ),
                    ("contains", [Yaml::Array(array), value]) => Yaml::Boolean(
                        array
                            .iter()
                            .any(|item| string(item).eq_ignore_ascii_case(&string(value))),
                    ),
                    ("contains", [Yaml::String(haystack), value]) => Yaml::Boolean(
                        haystack
                            .to_ascii_lowercase()
                            .contains(&string(value).to_ascii_lowercase()),
                    ),
                    _ => panic!("unsupported call {name}({args:?})"),
                }
            }
        }
    }
}

fn evaluate(source: &str, context: &BTreeMap<&str, Yaml>) -> bool {
    let mut parser = Parser(source);
    let expr = parser.expression(0);
    assert!(parser.0.trim().is_empty(), "unparsed: {}", parser.0);
    truthy(&expr.eval(context))
}

struct Event<'a> {
    association: &'a str,
    bot: bool,
    id: i64,
    ids: &'a str,
    draft: bool,
    command: Option<&'a str>,
    visibility: Option<&'a str>,
}

impl Default for Event<'_> {
    fn default() -> Self {
        Self {
            association: "NONE",
            bot: false,
            id: 42,
            ids: "",
            draft: false,
            command: None,
            visibility: Some("public"),
        }
    }
}

fn admits(event: Event<'_>) -> bool {
    let source = yaml(&read(REVIEW));
    let context = BTreeMap::from([
        (
            "github.event_name",
            Yaml::String(
                if event.command.is_some() {
                    "issue_comment"
                } else {
                    "pull_request_target"
                }
                .into(),
            ),
        ),
        (
            "github.event.pull_request.draft",
            Yaml::Boolean(event.draft),
        ),
        (
            "github.event.pull_request.author_association",
            Yaml::String(event.association.into()),
        ),
        (
            "github.event.pull_request.user.type",
            Yaml::String(if event.bot { "Bot" } else { "User" }.into()),
        ),
        ("github.event.pull_request.user.id", Yaml::Integer(event.id)),
        ("github.event.issue.pull_request", yaml("{}")),
        (
            "github.event.comment.body",
            Yaml::String(event.command.unwrap_or("").into()),
        ),
        (
            "github.event.repository.visibility",
            event
                .visibility
                .map_or(Yaml::Null, |v| Yaml::String(v.into())),
        ),
        (
            "github.event.repository.private",
            Yaml::Boolean(event.visibility != Some("public")),
        ),
        ("vars.REVIEW_BOT_IDS", Yaml::String(event.ids.into())),
    ]);
    evaluate(source["jobs"]["policy"]["if"].as_str().unwrap(), &context)
}

#[test]
fn evaluator_operators_literals_case_and_short_circuiting() {
    for (source, expected) in [
        ("'PuBlIc' == 'public' && !false", true),
        ("'Bot' != 'bot' || false", false),
        ("false && fromJSON('invalid[')", false),
        ("true || fromJSON('invalid[')", true),
        ("contains(fromJSON('[42,\"USER\"]'), '42')", true),
        ("startsWith('/REVIEW now', '/review')", true),
        ("(null || '') == '' && 42 != 0", true),
        ("contains('AbCd', 'BC')", true),
        ("'it''s' == 'IT''S'", true),
    ] {
        assert_eq!(evaluate(source, &BTreeMap::new()), expected, "{source}");
    }
}

#[test]
fn automatic_reviews_admit_trusted_associations_not_contributors_or_drafts() {
    for association in [
        "OWNER",
        "MEMBER",
        "COLLABORATOR",
        "CONTRIBUTOR",
        "FIRST_TIMER",
        "NONE",
        "owner",
    ] {
        assert_eq!(
            admits(Event {
                association,
                ..Event::default()
            }),
            ["OWNER", "MEMBER", "COLLABORATOR"]
                .iter()
                .any(|a| a.eq_ignore_ascii_case(association)),
            "{association}"
        );
        assert!(!admits(Event {
            association,
            draft: true,
            ..Event::default()
        }));
    }
}

#[test]
fn unassociated_bots_wait_unless_configured_trusted_associations_retain_admission() {
    for (bot, ids, expected) in [
        (true, "", false),
        (true, "[42]", true),
        (false, "[42]", false),
        (true, "[142]", false),
        (true, r#"["42"]"#, true),
        (true, "42", false),
    ] {
        assert_eq!(
            admits(Event {
                bot,
                ids,
                ..Event::default()
            }),
            expected,
            "{bot} {ids}"
        );
    }
    assert!(!admits(Event {
        bot: true,
        ids: "[42]",
        draft: true,
        ..Event::default()
    }));
    assert!(admits(Event {
        bot: true,
        association: "COLLABORATOR",
        ..Event::default()
    }));
}

#[test]
fn commands_reach_event_policy_retaining_bounds_and_write_role_check() {
    for (command, expected) in [
        ("/review", true),
        ("please /review", false),
        ("/REVIEW", true),
    ] {
        assert_eq!(
            admits(Event {
                command: Some(command),
                ..Event::default()
            }),
            expected
        );
    }
    let source = yaml(&read(REVIEW));
    let with = &source["jobs"]["policy"]["with"];
    assert_eq!(with["event"], Yaml::Boolean(true));
    assert!(
        with["allow"]
            .as_str()
            .unwrap()
            .contains("'workflow/review-command.toml' || 'workflow/review.toml'")
    );
    for (key, value) in [
        ("max-requests", "150"),
        ("budget", "500"),
        ("timeout", "75"),
    ] {
        assert_eq!(with[key].as_str(), Some(value));
    }
}

// Private repository opt-in remains off regardless of caller bounds.
#[test]
fn private_internal_or_missing_visibility_is_never_reviewed() {
    for visibility in [Some("private"), Some("internal"), None] {
        assert!(!admits(Event {
            association: "OWNER",
            visibility,
            ..Event::default()
        }));
        assert!(!admits(Event {
            command: Some("/review"),
            visibility,
            ..Event::default()
        }));
    }
    assert!(admits(Event {
        association: "OWNER",
        ..Event::default()
    }));
}

#[test]
fn only_admitted_agents_compete_and_only_pushes_cancel() {
    let source = yaml(&read(REVIEW));
    let agent = &source["jobs"]["agent"];
    assert_eq!(
        agent["if"].as_str(),
        Some("${{ needs.policy.outputs.admitted == 'true' }}")
    );
    assert_eq!(
        agent["concurrency"]["group"].as_str(),
        Some("review-pull-${{ github.event.pull_request.number || github.event.issue.number }}")
    );
    assert_eq!(
        agent["concurrency"]["cancel-in-progress"].as_str(),
        Some("${{ github.event_name == 'pull_request_target' }}")
    );
}

#[test]
fn trusted_source_one_network_join_before_dropping_root_no_post_agent_steps() {
    let text = read(REVIEW);
    excludes(
        &text,
        &[
            "pull_request.head",
            "head.ref",
            "secrets: inherit",
            "workflow_dispatch",
        ],
    );
    let source = yaml(&text);
    let jobs = &source["jobs"];
    let steps: Vec<_> = jobs["agent"]["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .map(|step| {
            let (key, value) = step.as_hash().unwrap().iter().next().unwrap();
            assert!(["uses", "name", "id"].contains(&key.as_str().unwrap()));
            value.as_str().unwrap()
        })
        .collect();
    assert_eq!(
        steps,
        [
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "The policy call's source repository",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "Create the job directory",
            "./.agentic-job-source/prepare",
            "Keep the job inputs private",
            "Join the tailnet",
            "Check inference proxy reachability",
            "The host's configuration",
            "./.agentic-job-source/secure-host",
            "run",
        ]
    );
    let agent = job(&text, "agent");
    includes(agent, &["ref: ${{ needs.policy.outputs.ref }}"]);
    excludes(
        agent,
        &[
            "pull-requests: write",
            "contents: write",
            "issues: write",
            "secrets.",
        ],
    );
    for name in ["policy", "check", "apply"] {
        includes(job(&text, name), &[&format!("/{name}.yml@{PIN}")]);
    }
    for name in jobs
        .as_hash()
        .unwrap()
        .keys()
        .map(|key| key.as_str().unwrap())
    {
        if name != "apply" {
            excludes(job(&text, name), &["secrets."]);
        }
    }
}

#[test]
fn both_wrappers_retain_release_pins_and_check_proxy_between_join_and_setup() {
    let (sha, version) = PIN.split_once(" # v").unwrap();
    assert_eq!(sha.len(), 40);
    assert!(
        sha.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let parts: Vec<_> = version.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert!(
        parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    );
    for file in [DISPATCH, REVIEW] {
        let text = read(file);
        let calls: Vec<_> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("uses: "))
            .filter(|call| {
                ["/policy.yml@", "/check.yml@", "/apply.yml@"]
                    .iter()
                    .any(|part| call.contains(part))
            })
            .collect();
        assert_eq!(calls.len(), 3, "{file}");
        assert_eq!(
            text.matches("uses: cgwalters-forge/agentic-job/").count(),
            3,
            "{file}"
        );
        for call in calls {
            assert!(call.starts_with("cgwalters-forge/agentic-job/"), "{file}");
            assert_eq!(call.split_once('@').unwrap().1, PIN, "{file}");
        }
        let source = yaml(&text);
        assert_eq!(
            source["jobs"]["policy"]["permissions"]["actions"].as_str(),
            Some("read")
        );
        let agent = job(&text, "agent");
        let check = "run: agentic-job secure-host inference /var/tmp/agentic-job/config.toml";
        assert_eq!(agent.matches(check).count(), 1);
        let join = agent.find("name: Join the tailnet").unwrap();
        let inference = agent.find(check).unwrap();
        let host = agent
            .find("uses: ./.agentic-job-source/secure-host")
            .unwrap();
        let run = agent.find("uses: ./.agentic-job-source/run").unwrap();
        assert!(join < inference && inference < host && host < run, "{file}");
        excludes(agent, &["steps.secure-host.outputs.session-endpoint"]);
    }
}

// A second user prevents the agent minting tokens using the runner's OIDC capability.
#[test]
fn both_agents_use_forwarder_public_binary_private_inputs_and_proposals_digest() {
    for file in [DISPATCH, REVIEW] {
        let text = read(file);
        let agent = job(&text, "agent");
        includes(
            agent,
            &[
                "run: install -d -m 0711 /var/tmp/agentic-job",
                "chmod 0600 /var/tmp/agentic-job/*\n          chmod 0755 /var/tmp/agentic-job/agentic-job",
                "run: agentic-job config --host --from /var/tmp/agentic-job/config.toml --string sandbox.env.PATH=/var/tmp/agentic-job:/usr/local/bin:/usr/bin:/bin > \"$RUNNER_TEMP/agentic-job-host.toml\"",
                "config: ${{ runner.temp }}/agentic-job-host.toml",
                "agent-user: sandbox\n",
                "\n      safe-outputs-artifact-digest: ${{ steps.run.outputs.safe-outputs-artifact-digest }}\n",
            ],
        );
        assert_eq!(agent.matches("job-dir: /var/tmp/agentic-job\n").count(), 2);
        let source = yaml(&text);
        let hosts: Vec<_> = source["jobs"]["agent"]["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .filter_map(|step| step["uses"].as_str())
            .filter(|uses| uses.starts_with("./.agentic-job-source/secure-host"))
            .collect();
        assert_eq!(hosts, ["./.agentic-job-source/secure-host"]);
        excludes(
            agent,
            &["runner-home:", "--no-script", "secure-host/internal"],
        );
        includes(
            job(&text, "check"),
            &[
                "\n      safe-outputs-artifact-digest: ${{ needs.agent.outputs.safe-outputs-artifact-digest }}",
            ],
        );
    }
}

#[test]
fn steering_receives_dispatch_item_through_policy_metadata_or_review_event() {
    let dispatch = read(DISPATCH);
    includes(
        job(&dispatch, "policy"),
        &[
            "      issue: ${{ inputs.item }}\n",
            "      output-repo: ${{ inputs.repo }}\n",
        ],
    );
    let review = read(REVIEW);
    includes(job(&review, "policy"), &["      event: true\n"]);
    for source in [&dispatch, &review] {
        excludes(source, &["watch-item:"]);
    }
}

#[test]
fn runner_smoke_uses_same_policy_binary_and_host_sequence_as_wrappers() {
    let text = read(".github/workflows/secure-host.yml");
    includes(&text, &[&format!("policy.yml@{PIN}")]);
    let policy = job(&text, "wrapper-policy");
    includes(policy, &["max-requests: '0'", "budget: '1'"]);
    assert_eq!(
        yaml(&text)["jobs"]["wrapper-policy"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    let smoke = job(&text, "wrapper-sandbox");
    includes(smoke, &[&format!("ref: {PIN}")]);
    let mut previous = None;
    for step in [
        "install -d -m 0711 /var/tmp/agentic-job",
        "uses: ./.agentic-job-source/prepare",
        "job-dir: /var/tmp/agentic-job",
        "chmod 0600 /var/tmp/agentic-job/*",
        "chmod 0755 /var/tmp/agentic-job/agentic-job",
        "agentic-job secure-host inference /var/tmp/agentic-job/config.toml",
        "agentic-job config --host --from /var/tmp/agentic-job/config.toml --string sandbox.env.PATH=/var/tmp/agentic-job:/usr/local/bin:/usr/bin:/bin",
        "uses: ./.agentic-job-source/secure-host",
        "python3 workflow/sandbox-binary-check.py",
    ] {
        let index = smoke.find(step).unwrap_or_else(|| panic!("missing {step}"));
        assert!(previous.is_none_or(|p| index > p), "{step}");
        previous = Some(index);
    }
    excludes(
        smoke,
        &[
            "continue-on-error:",
            "check: false",
            "check: 'false",
            "check: \"false",
            "secrets.",
        ],
    );
    includes(
        &read("workflow/sandbox-binary-check.py"),
        &[
            "\"helper\", \"enter\"",
            "exec agentic-job --version",
            "test \"$(id -u)\" = \"$1\"",
        ],
    );
}

#[test]
fn review_bounds_scope_forge_tables_to_github() {
    for file in ["review.toml", "review-command.toml"] {
        let out = Command::new("python3")
            .args([
                "-c",
                "import json, sys, tomllib; print(json.dumps(tomllib.load(sys.stdin.buffer)))",
            ])
            .stdin(Stdio::from(
                fs::File::open(root().join("workflow").join(file)).unwrap(),
            ))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{file}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let bounds = yaml(std::str::from_utf8(&out.stdout).unwrap());
        assert!(bounds["outputs"].is_badvalue(), "{file}");
        assert!(bounds["trigger"].is_badvalue(), "{file}");
        assert_eq!(
            bounds["github"]["outputs"],
            yaml(r#"{"add_comment":{"max":1},"noop":{"max":1}}"#),
            "{file}"
        );
        assert!(
            !bounds["github"]["trigger"]["events"]
                .as_vec()
                .unwrap()
                .is_empty(),
            "{file}"
        );
    }
}

#[test]
fn runner_probe_checks_identity_binary_execution_output_bounds_and_failures() {
    let out = Command::new("python3")
        .args(["-B", "workflow/sandbox-binary-check.test.py"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn real_reviews_are_opt_in_and_use_only_checked_artifact_at_admitted_head() {
    let text = read(REVIEW);
    includes(
        job(&text, "submit"),
        &[
            "vars.AGENT_REAL_REVIEWS == 'true'",
            "needs.check.result == 'success' && needs.check.outputs.artifact-id != ''",
            "artifact-ids: ${{ needs.check.outputs.artifact-id }}",
            "HEAD_SHA: ${{ needs.policy.outputs.review-head }}",
            "review submit --accepted \"$RUNNER_TEMP/checked/agent_output.json\" --output \"$GITHUB_OUTPUT\"",
            "review-url: ${{ steps.submit.outputs.review-url }}",
        ],
    );
    let apply = yaml(&text)["jobs"]["apply"]["if"]
        .as_str()
        .unwrap()
        .to_owned();
    let cancelled = apply.find("!cancelled()").unwrap();
    assert!(cancelled < apply.find("needs.submit.outputs.review-url == ''").unwrap());
    includes(
        job(&text, "status"),
        &[
            "needs.agent.result != 'cancelled'",
            "\"$RUNNER_TEMP/bin/agentic-job\" review status",
        ],
    );
}

#[test]
fn publication_uses_policy_binary_not_removed_javascript_helpers() {
    let review = read(REVIEW);
    let dispatch = read(DISPATCH);
    let session = job(&dispatch, "session");
    for body in [
        job(&review, "refusal"),
        job(&review, "submit"),
        job(&review, "status"),
        session,
    ] {
        includes(
            body,
            &[
                "needs.policy.outputs.binary-artifact-id != ''",
                "artifact-ids: ${{ needs.policy.outputs.binary-artifact-id }}",
                "chmod +x \"$RUNNER_TEMP/bin/agentic-job\"",
            ],
        );
        excludes(body, &["actions/checkout", "actions/github-script"]);
        assert!(
            !body
                .lines()
                .any(|line| line.contains("workflow/") && line.contains(".cjs"))
        );
    }
    includes(
        job(&review, "refusal"),
        &["TEXT: ${{ needs.policy.outputs.refusal }}"],
    );
    for body in [job(&review, "refusal"), session] {
        includes(body, &["\"$RUNNER_TEMP/bin/agentic-job\" status"]);
    }
}
