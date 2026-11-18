//! Review admission and publication boundaries; no live forge or tailnet calls.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use yaml_rust2::{Yaml, YamlLoader};

// What both wrappers call, by immutable commit with its tag as a comment: a
// release, or, when a caller needs an unreleased fix, a commit of main with
// the tag of its CI build (`build-DIGEST`), which zizmor's ref-version-mismatch
// audit resolves. A bump changes this, the six calls, the smoke job's two and
// the README.
const PIN: &str = "740ff7ff4dcef59dcb0619eb14d0e357c3193f62 # v0.6.15";
const PROSE_PIN: &str = PIN;
const REVIEW: &str = ".github/workflows/cgwalters-review.yml";
const DISPATCH: &str = ".github/workflows/cgwalters-agentic-job.yml";
// Both wrappers' agent job, called by local path.
const AGENT: &str = ".github/workflows/cgwalters-agent.yml";
const SMOKE: &str = ".github/workflows/secure-host.yml";
const LOCAL_HOST: &str = "./.agentic-job-source/secure-host";

// Moving a tag or substituting another action must not change privileged setup.
// Normalize only the validated host call so all existing assertions cover both forms.
fn host_source(text: &str, name: &str) -> String {
    let source = yaml(text);
    let hosts: Vec<_> = source["jobs"][name]["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .filter(|step| {
            step["with"]["config"].as_str() == Some("${{ runner.temp }}/agentic-job-host.toml")
        })
        .collect();
    assert_eq!(hosts.len(), 1);
    let host = hosts[0]["uses"].as_str().unwrap();
    assert!(
        host == LOCAL_HOST
            || host
                .strip_prefix("cgwalters-forge/actions/secure-host@")
                .is_some_and(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())),
        "untrusted or mutable host reference: {host}"
    );
    text.replace(host, LOCAL_HOST)
}

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
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.' && c != '-')
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
                    ("cancelled", []) => context
                        .get("cancelled")
                        .cloned()
                        .unwrap_or(Yaml::Boolean(false)),
                    ("always", []) => Yaml::Boolean(true),
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

fn value(source: &str, context: &BTreeMap<&str, Yaml>) -> Yaml {
    let mut parser = Parser(source);
    let expr = parser.expression(0);
    assert!(parser.0.trim().is_empty(), "unparsed: {}", parser.0);
    expr.eval(context)
}

fn evaluate(source: &str, context: &BTreeMap<&str, Yaml>) -> bool {
    truthy(&value(source, context))
}

// A session is triage under another name: every policy input that names
// no kind in its text resolves as triage's does, so it is an analysis run
// with comment outputs and the same bounds, caps and publication.
#[test]
fn session_dispatch_gets_triage_policy_inputs() {
    let workflow = yaml(&read(DISPATCH));
    let with = workflow["jobs"]["policy"]["with"].as_hash().unwrap();
    let resolve = |kind: &str| -> BTreeMap<String, String> {
        let context = BTreeMap::from([
            ("inputs.kind", Yaml::String(kind.to_owned())),
            // The scripted agent, so fake-profile resolves too.
            (
                "needs.target.outputs.agent",
                Yaml::String("fake".to_owned()),
            ),
        ]);
        with.iter()
            .filter_map(|(key, source)| {
                let expr = source
                    .as_str()?
                    .trim()
                    .strip_prefix("${{")?
                    .strip_suffix("}}")?;
                (!expr.contains("format(") && !expr.contains("${{")).then(|| {
                    (
                        key.as_str().unwrap().to_owned(),
                        string(&value(expr, &context)),
                    )
                })
            })
            .collect()
    };
    let session = resolve("session");
    assert_eq!(session, resolve("triage"));
    assert_eq!(session["kind"], "analysis");
    assert_eq!(
        session["outputs"],
        "add_comment,noop,missing_tool,missing_data"
    );
    assert_eq!(session["max-outputs"], "3");
    assert_eq!(session["fake-profile"], "comment");
    assert_eq!(session["apply-partial"], "true");
    assert_eq!(session["notify"], "comment");
}

// Only triage, and only when the caller opts in, asks policy for `all`:
// the output types its allow file grants. Every other kind, and triage by
// default, keeps its fixed list; the target job refuses the input elsewhere.
#[test]
fn triage_outputs_from_allow_widens_only_triage() {
    let workflow = yaml(&read(DISPATCH));
    let policy = &workflow["jobs"]["policy"]["with"];
    let expr = |key: &str| {
        policy[key]
            .as_str()
            .unwrap()
            .trim()
            .strip_prefix("${{")
            .unwrap()
            .strip_suffix("}}")
            .unwrap()
            .to_owned()
    };
    let fixed = [
        ("review", "add_comment,noop"),
        (
            "implement",
            "create_pull_request,noop,missing_tool,missing_data",
        ),
        (
            "fix",
            "push_to_pull_request_branch,noop,missing_tool,missing_data",
        ),
        ("triage", "add_comment,noop,missing_tool,missing_data"),
        ("research", "add_comment,noop,missing_tool,missing_data"),
        ("session", "add_comment,noop,missing_tool,missing_data"),
    ];
    for (kind, want) in fixed {
        for mode in [None, Some(false), Some(true)] {
            let mut context = BTreeMap::from([("inputs.kind", Yaml::String(kind.into()))]);
            if let Some(mode) = mode {
                context.insert("inputs.triage-outputs-from-allow", Yaml::Boolean(mode));
            }
            let want = if kind == "triage" && mode == Some(true) {
                "all"
            } else {
                want
            };
            assert_eq!(
                string(&value(&expr("outputs"), &context)),
                want,
                "{kind} {mode:?}"
            );
            // The cap stays as it was, whatever the bounds allow.
            assert_eq!(
                string(&value(&expr("max-outputs"), &context)),
                if kind == "review" { "1" } else { "3" },
                "{kind} {mode:?}"
            );
        }
    }
}

#[test]
fn research_artifact_only_never_applies_with_or_without_a_pat() {
    let workflow = yaml(&read(DISPATCH));
    let jobs = &workflow["jobs"];
    let resolve = |source: &Yaml, context: &BTreeMap<&str, Yaml>| {
        let expr = source.as_str().unwrap().trim();
        value(
            expr.strip_prefix("${{")
                .unwrap()
                .strip_suffix("}}")
                .unwrap(),
            context,
        )
    };
    for kind in [
        "implement",
        "triage",
        "research",
        "session",
        "review",
        "fix",
    ] {
        for mode in [None, Some(false), Some(true)] {
            for pat in ["", "fixture-pat"] {
                let mut context = BTreeMap::from([
                    ("inputs.kind", Yaml::String(kind.into())),
                    ("needs.policy.result", Yaml::String("success".into())),
                    ("inputs.apply-environment", Yaml::String("apply".into())),
                    ("secrets.SAFE_OUTPUTS_PAT", Yaml::String(pat.into())),
                ]);
                if let Some(mode) = mode {
                    context.insert("inputs.research-artifact-only", Yaml::Boolean(mode));
                }
                let only = mode == Some(true);
                assert_eq!(
                    truthy(&resolve(&jobs["apply"]["if"], &context)),
                    !only && kind != "review"
                );
                let policy = &jobs["policy"]["with"];
                assert_eq!(
                    string(&resolve(&policy["notify"], &context)),
                    if only || kind == "review" {
                        "none"
                    } else {
                        "comment"
                    }
                );
                assert_eq!(
                    string(&resolve(&policy["apply-environment"], &context)),
                    if only || kind == "review" {
                        ""
                    } else {
                        "apply"
                    }
                );
                assert_eq!(
                    string(&resolve(
                        &jobs["policy"]["secrets"]["SAFE_OUTPUTS_PAT"],
                        &context
                    )),
                    if only || kind == "review" { "" } else { pat }
                );
                assert_eq!(
                    truthy(&resolve(&policy["apply-partial"], &context)),
                    !only && ["implement", "research", "triage", "session"].contains(&kind)
                );
                assert_eq!(
                    truthy(&resolve(&jobs["research-handback"]["if"], &context)),
                    only && kind == "research"
                );
                context.insert("cancelled", Yaml::Boolean(true));
                assert!(!truthy(&resolve(
                    &jobs["research-handback"]["if"],
                    &context
                )));
            }
        }
    }
}

#[test]
fn ordinary_review_submit_fails_closed_and_never_enters_pat_apply() {
    let workflow = yaml(&read(DISPATCH));
    let condition = workflow["jobs"]["submit"]["if"].as_str().unwrap();
    let condition = condition
        .trim()
        .strip_prefix("${{")
        .unwrap()
        .strip_suffix("}}")
        .unwrap();
    let context: BTreeMap<_, _> = [
        ("inputs.kind", "review"),
        ("vars.AGENT_REAL_REVIEWS", "true"),
        ("needs.policy.outputs.review-head", "head"),
        ("needs.policy.outputs.binary-artifact-id", "123"),
        ("needs.agent.result", "success"),
        ("needs.agent.outputs.exit", "0"),
        ("needs.check.result", "success"),
        ("needs.check.outputs.artifact-id", "456"),
        ("needs.check.outputs.verdict", "APPROVE"),
    ]
    .into_iter()
    .map(|(k, v)| (k, Yaml::String(v.to_owned())))
    .collect();
    assert!(evaluate(condition, &context));
    for (key, values) in [
        (
            "inputs.kind",
            vec!["implement", "fix", "triage", "research", "session", ""],
        ),
        ("vars.AGENT_REAL_REVIEWS", vec!["", "false"]),
        ("needs.policy.outputs.review-head", vec![""]),
        ("needs.policy.outputs.binary-artifact-id", vec![""]),
        (
            "needs.agent.result",
            vec!["failure", "cancelled", "skipped"],
        ),
        ("needs.agent.outputs.exit", vec!["", "1", "124"]),
        (
            "needs.check.result",
            vec!["failure", "cancelled", "skipped"],
        ),
        ("needs.check.outputs.artifact-id", vec![""]),
        ("needs.check.outputs.verdict", vec!["", "NOOP", "REJECT"]),
    ] {
        for value in values {
            let mut changed = context.clone();
            changed.insert(key, Yaml::String(value.to_owned()));
            assert!(!evaluate(condition, &changed), "{key}={value}");
        }
    }
    let mut changed = context.clone();
    changed.insert("cancelled", Yaml::Boolean(true));
    assert!(!evaluate(condition, &changed));
    changed = context.clone();
    changed.insert(
        "needs.check.outputs.verdict",
        Yaml::String("CHANGES".to_owned()),
    );
    assert!(evaluate(condition, &changed));
    let apply = workflow["jobs"]["apply"]["if"].as_str().unwrap();
    let apply = apply
        .trim()
        .strip_prefix("${{")
        .unwrap()
        .strip_suffix("}}")
        .unwrap();
    changed.insert("needs.policy.result", Yaml::String("success".to_owned()));
    assert!(!evaluate(apply, &changed));
    for kind in ["implement", "fix", "triage", "research", "session"] {
        changed.insert("inputs.kind", Yaml::String(kind.to_owned()));
        assert!(evaluate(apply, &changed));
    }
}

#[test]
fn ordinary_review_privileged_dependencies_are_exclusive_literal_pins() {
    let workflow = yaml(&read(DISPATCH));
    for (job, action) in [("target", "admit"), ("target", "deployment")] {
        let prefix = format!("jmarrero-forge/actions/{action}@");
        let steps: Vec<_> = workflow["jobs"][job]["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .filter(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|s| s.starts_with(&prefix))
            })
            .collect();
        assert_eq!(steps.len(), 2);
        for kind in [
            "review",
            "implement",
            "fix",
            "triage",
            "research",
            "session",
        ] {
            let context = BTreeMap::from([
                ("inputs.kind", Yaml::String(kind.to_owned())),
                (
                    "needs.target.outputs.agent",
                    Yaml::String("opencode".to_owned()),
                ),
            ]);
            let active: Vec<_> = steps
                .iter()
                .filter(|step| {
                    let expr = step["if"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("${{")
                        .unwrap()
                        .strip_suffix("}}")
                        .unwrap();
                    evaluate(expr, &context)
                })
                .collect();
            assert_eq!(active.len(), 1, "{kind}/{action}");
            let reference = active[0]["uses"]
                .as_str()
                .unwrap()
                .strip_prefix(&prefix)
                .unwrap();
            // Unlike upstream's, ours pins every kind (see the wrapper).
            assert_eq!(reference.len(), 40, "{kind}");
            assert!(reference.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
    // The tailnet join is privileged for every kind: one pinned step in
    // each wrapper's register job, the independent prose register job and
    // the agent job both wrappers call at their own commit, and nowhere else.
    for (file, name, count) in [
        (DISPATCH, "register", 1),
        (REVIEW, "register", 2),
        (REVIEW, "prose-register", 2),
        (AGENT, "agent", 1),
    ] {
        assert_eq!(read(file).matches("tailscale@").count(), count, "{file}");
        let source = yaml(&read(file));
        let joins: Vec<_> = source["jobs"][name]["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .filter(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|s| s.contains("tailscale"))
            })
            .collect();
        assert_eq!(joins.len(), 1, "{file}");
        assert!(joins[0]["if"].is_badvalue(), "{file}");
        let reference = joins[0]["uses"]
            .as_str()
            .unwrap()
            .strip_prefix("jmarrero-forge/actions/tailscale@")
            .unwrap();
        assert_eq!(reference.len(), 40, "{file}");
        assert!(reference.bytes().all(|b| b.is_ascii_hexdigit()), "{file}");
    }
}

// Until policy/check/submit support the structured classifier contract, an
// ordinary verdict writer must not impersonate the consumer's reserved job.
#[test]
fn draft_classifier_writer_name_is_not_claimed_by_ordinary_workflows() {
    let reserved = "agent-board-review-result-v1";
    for entry in fs::read_dir(root().join(".github/workflows")).unwrap() {
        let path = entry.unwrap().path();
        if !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        // Check the raw text too: an expression or reusable caller name must
        // not hide an ordinary writer behind this provenance contract.
        assert!(!text.contains(reserved), "{}", path.display());
    }
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
    review_source(&read(REVIEW));
    agent_source(&read(AGENT));
}

// The review wrapper: no head, no inherited secret, the pinned calls, and
// secrets only for apply and for the agent's call, which gets the register
// job's two values and nothing else.
fn review_source(text: &str) {
    excludes(
        text,
        &[
            "pull_request.head",
            "head.ref",
            "secrets: inherit",
            "workflow_dispatch",
        ],
    );
    let source = yaml(text);
    let jobs = &source["jobs"];
    for name in ["policy", "check", "apply"] {
        includes(job(text, name), &[&format!("/{name}.yml@{PIN}")]);
    }
    calls_register_then_agent(text);
    for name in jobs
        .as_hash()
        .unwrap()
        .keys()
        .map(|key| key.as_str().unwrap())
    {
        if !["apply", "agent"].contains(&name) {
            excludes(job(text, name), &["secrets."]);
        }
    }
}

/// Both wrappers' register and agent jobs: the register job is the
/// wrapper's own, as the token's `job_workflow_ref` names the file that
/// defines the job, and holds the identity token alone; the agent's call,
/// of the shared workflow at the wrapper's commit, reads only and gets
/// exactly register's two outputs.
fn calls_register_then_agent(text: &str) {
    let source = yaml(text);
    let jobs = &source["jobs"];
    register_job(text);
    let agent = &jobs["agent"];
    assert_eq!(
        agent["uses"].as_str(),
        Some("./.github/workflows/cgwalters-agent.yml")
    );
    assert!(
        agent["needs"]
            .as_vec()
            .unwrap()
            .contains(&Yaml::String("register".into()))
    );
    assert_eq!(
        agent["permissions"],
        yaml("contents: read\nissues: read\npull-requests: read\nactions: read")
    );
    assert_eq!(
        agent["secrets"],
        yaml(
            "INFERENCE_RUN_TOKEN: ${{ needs.register.outputs.run-token }}\nTAILNET_KEY: ${{ needs.register.outputs.tailnet-key }}"
        )
    );
    assert_eq!(
        agent["with"]["policy"].as_str(),
        Some("${{ toJSON(needs.policy.outputs) }}")
    );
    let grants = text
        .lines()
        .filter(|line| line.trim() == "id-token: write")
        .count();
    let expected = if jobs["prose-register"].is_badvalue() {
        2
    } else {
        4
    };
    assert_eq!(grants, expected, "register's and check's, including prose");
}

/// The shared agent job: upstream agent.yml's steps with our join, as the
/// runner's user, with no identity token, nothing written and only the
/// call's two secrets, each in the one step that spends it.
fn agent_source(text: &str) {
    let text = host_source(text, "agent");
    excludes(
        &text,
        &[
            "pull_request.head",
            "head.ref",
            "secrets: inherit",
            "id-token:",
            "vars.",
        ],
    );
    let source = yaml(&text);
    let steps: Vec<_> = source["jobs"]["agent"]["steps"]
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
            "Install RHEL prerequisites",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "The policy call's source repository",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "./.agentic-job-source/prepare",
            "Admit the runner-user configuration",
            "Check the tailnet key is this attempt's and unexpired",
            "Join the tailnet",
            "Check inference proxy reachability",
            "The host's configuration",
            "Add last-resort Ubuntu mirrors",
            "Prepare the agents and trusted helper",
            "./.agentic-job-source/secure-host",
            "The setup script",
            "Prepare bounded prose and pinned checklist",
            "run",
        ]
    );
    assert_eq!(
        source["jobs"]["agent"]["permissions"],
        yaml("contents: read\nissues: read\npull-requests: read\nactions: read")
    );
    let agent = job(&text, "agent");
    includes(agent, &["ref: ${{ fromJSON(inputs.policy).ref }}"]);
    excludes(
        agent,
        &["pull-requests: write", "contents: write", "issues: write"],
    );
    assert_eq!(
        captures(agent, "secrets."),
        ["TAILNET_KEY }}", "INFERENCE_RUN_TOKEN }}"]
    );
    let order = [
        "name: Join the tailnet",
        "authkey: ${{ secrets.TAILNET_KEY }}",
        "run: agentic-job secure-host inference .agentic-job/config.toml",
        "--string \"host.env.PATH=$RUNNER_TEMP/agentic-job-bin:",
        "uses: ./.agentic-job-source/secure-host",
        "run: python3 .agentic-job-source/workflow/runner-setup.py setup",
        "uses: ./.agentic-job-source/run",
        "inference-run-token: ${{ secrets.INFERENCE_RUN_TOKEN }}",
    ];
    let positions: Vec<_> = order
        .iter()
        .map(|part| {
            assert_eq!(agent.matches(part).count(), 1, "{part}");
            agent.find(part).unwrap()
        })
        .collect();
    assert!(positions.is_sorted(), "{order:?}");
}

fn captures<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
    text.match_indices(prefix)
        .map(|(i, _)| {
            let rest = &text[i + prefix.len()..];
            &rest[..rest.find('\n').unwrap_or(rest.len())]
        })
        .collect()
}

// The register job: a hosted runner, the identity token and nothing else,
// the policy call's own uploads, a join that mints the agent job's key, and
// `agentic-job register`, whose output and the key leave it as job outputs.
#[test]
fn register_job_is_hosted_and_holds_the_only_identity_token() {
    for file in [DISPATCH, REVIEW] {
        register_job(&read(file));
    }
}

fn register_job(text: &str) {
    let body = job(text, "register");
    excludes(
        body,
        &[
            "secrets.",
            "actions/checkout",
            ".agentic-job-source",
            "uses: ./",
        ],
    );
    let source = yaml(text);
    let register = &source["jobs"]["register"];
    assert!(register["uses"].is_badvalue());
    assert_eq!(register["runs-on"].as_str(), Some("ubuntu-26.04"));
    assert_eq!(register["permissions"], yaml("id-token: write"));
    assert_eq!(
        register["if"].as_str(),
        Some("${{ needs.policy.outputs.admitted == 'true' }}")
    );
    assert_eq!(
        register["outputs"],
        yaml(
            "run-token: ${{ steps.register.outputs.run-token }}\ntailnet-key: ${{ steps.tailnet.outputs.key }}\n\
             tailnet-key-attempt: ${{ github.run_attempt }}\ntailnet-key-minted-at: ${{ steps.minted.outputs.at }}"
        )
    );
    let steps: Vec<_> = register["steps"]
        .as_vec()
        .unwrap()
        .iter()
        .map(|step| {
            step["name"]
                .as_str()
                .or(step["uses"].as_str())
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        steps,
        [
            "The policy call's source and uploads",
            "actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333",
            "Refuse unsupported child agents before credentials",
            "The key's mint time",
            "Join the tailnet and mint the agent job's key",
            "Register the run at the inference proxy",
        ]
    );
    includes(
        body,
        &[
            "EXPECTED_REPOSITORY: cgwalters-forge/agentic-job",
            "[[ \"$BINARY\" =~ ^[1-9][0-9]*$ && \"$POLICY\" =~ ^[1-9][0-9]*$ ]] ||",
            "mint: 'true'",
            "artifact-ids: ${{ needs.policy.outputs.binary-artifact-id }},${{ needs.policy.outputs.policy-artifact-id }}",
            "\"$JOB_DIR/agentic-job\" register --config \"$JOB_DIR/config.toml\" --output \"$GITHUB_OUTPUT\"",
        ],
    );
}

#[test]
fn both_wrappers_retain_release_pins_and_check_proxy_between_join_and_setup() {
    let (sha, comment) = PIN.split_once(" # ").unwrap();
    let version = comment.strip_prefix('v').unwrap_or(comment);
    assert_eq!(sha.len(), 40);
    assert!(
        sha.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    // vX.Y.Z, or a CI build's tag, build-DIGEST.
    let build = version.strip_prefix("build-");
    if let Some(digest) = build {
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    } else {
        let parts: Vec<_> = version.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(
            parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        );
    }
    for file in [DISPATCH, REVIEW] {
        wrapper_release_and_order(&read(file));
    }
}

fn wrapper_release_and_order(text: &str) {
    let file = "wrapper fixture";
    let calls: Vec<_> = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("uses: "))
        .filter(|call| {
            ["/policy.yml@", "/check.yml@", "/apply.yml@"]
                .iter()
                .any(|part| call.contains(part))
        })
        .collect();
    let has_prose = text.contains("\n  prose-policy:\n");
    assert_eq!(calls.len(), if has_prose { 6 } else { 3 }, "{file}");
    assert_eq!(
        text.matches("uses: cgwalters-forge/agentic-job/").count(),
        if has_prose { 6 } else { 3 },
        "{file}"
    );
    for (index, call) in calls.iter().enumerate() {
        assert!(call.starts_with("cgwalters-forge/agentic-job/"), "{file}");
        assert_eq!(
            call.split_once('@').unwrap().1,
            if index < 3 { PIN } else { PROSE_PIN },
            "{file}"
        );
    }
    let source = yaml(text);
    assert_eq!(
        source["jobs"]["policy"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    calls_register_then_agent(text);
    excludes(
        text,
        &[
            "agent-user",
            "runner-home",
            "secure-host inference",
            "session-endpoint",
        ],
    );
}

// The agent is the runner's user (agentic-job#2989): the host's file has no
// setup.script, which the runner's user runs after secure-host; `run` gets
// its home and the register job's run token, and no key, MCP credential or
// second user.
#[test]
fn both_agents_use_trusted_host_public_binary_private_inputs_and_proposals_digest() {
    agent_inputs(&read(AGENT));
    for file in [DISPATCH, REVIEW] {
        includes(
            job(&read(file), "check"),
            &[
                "\n      safe-outputs-artifact-digest: ${{ needs.agent.outputs.safe-outputs-artifact-digest }}",
            ],
        );
    }
}

fn agent_inputs(text: &str) {
    let text = host_source(text, "agent");
    let agent = job(&text, "agent");
    includes(
        agent,
        &[
            "install -D -m 0755 .agentic-job/agentic-job \"$RUNNER_TEMP/agentic-job-bin/agentic-job\"\n",
            "agentic-job config --host --prepared --from .agentic-job/config.toml --string \"host.env.PATH=$RUNNER_TEMP/agentic-job-bin:/usr/local/bin:/usr/bin:/bin\" > \"$RUNNER_TEMP/agentic-job-host.toml\"\n",
            "config: ${{ runner.temp }}/agentic-job-host.toml",
            "AGENT_USER: runner\n",
            "run: python3 .agentic-job-source/workflow/runner-setup.py admit",
            "runner-home: ${{ runner.temp }}/agent-home\n",
            "AGENT_HOME: ${{ runner.temp }}/agent-home\n",
            "\n      safe-outputs-artifact-digest: ${{ steps.run.outputs.safe-outputs-artifact-digest }}\n",
        ],
    );
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
        &[
            "agent-user:",
            "job-dir:",
            "/var/tmp/agentic-job",
            "sandbox.env",
            "--no-script",
            "secure-host/internal",
            "inference-api-key",
            "mcp-token",
            "check: false",
        ],
    );
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
    smoke_source(&read(SMOKE));
}

#[test]
fn wrappers_forward_the_configured_write_policy_to_the_host_action() {
    for (file, name) in [(AGENT, "agent"), (SMOKE, "wrapper-runner")] {
        let source = yaml(&read(file));
        let steps = source["jobs"][name]["steps"].as_vec().unwrap();
        let config = steps
            .iter()
            .find(|step| step["id"].as_str() == Some("host-config"))
            .unwrap();
        let script = config["run"].as_str().unwrap();
        assert!(script.contains("config --host --prepared --from"));
        let select = script.lines().last().unwrap();
        let host = steps
            .iter()
            .find(|step| {
                step["uses"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("cgwalters-forge/actions/secure-host@"))
            })
            .unwrap();
        assert_eq!(
            host["with"]["egress"].as_str(),
            Some("${{ steps.host-config.outputs.egress }}")
        );
        for (text, expected) in [
            ("[egress]\nwrite-policy = true\n", Some("write-policy")),
            ("[egress]\nwrite-policy = false\n", Some("off")),
            ("[egress]\nproxy = true\n", Some("off")),
            ("# write-policy = true\n", Some("off")),
            (
                "[egress]\n\"write-policy\" = true # restricted\n",
                Some("write-policy"),
            ),
            ("[egress\n", None),
        ] {
            let temp = tempfile::tempdir().unwrap();
            fs::write(temp.path().join("agentic-job-host.toml"), text).unwrap();
            let output = temp.path().join("outputs");
            let result = Command::new("bash")
                .args(["-e", "-o", "pipefail", "-c", select])
                .env("RUNNER_TEMP", temp.path())
                .env("GITHUB_OUTPUT", &output)
                .output()
                .unwrap();
            assert_eq!(
                result.status.success(),
                expected.is_some(),
                "{file}: {text}"
            );
            assert_eq!(
                fs::read_to_string(output).unwrap(),
                expected
                    .map(|mode| format!("egress={mode}\n"))
                    .unwrap_or_default(),
                "{file}: {text}"
            );
        }
    }
}

fn smoke_source(text: &str) {
    let text = host_source(text, "wrapper-runner");
    includes(&text, &[&format!("policy.yml@{PIN}")]);
    let policy = job(&text, "wrapper-policy");
    // A scripted dispatch: upstream's comment session, no model or broker.
    includes(
        policy,
        &[
            "max-requests: '0'",
            "budget: '1'",
            "      agent: fake\n",
            "      fake-profile: comment\n",
            "      outputs: add_comment,noop\n",
            "      max-outputs: '1'\n",
            "      notify: none\n",
        ],
    );
    excludes(
        policy,
        &["inference-", "apply-environment", "setup:", "secrets"],
    );
    assert_eq!(
        yaml(&text)["jobs"]["wrapper-policy"]["permissions"]["actions"].as_str(),
        Some("read")
    );
    let smoke = job(&text, "wrapper-runner");
    includes(smoke, &[&format!("ref: {PIN}")]);
    // The agent job's own sequence, the same lines.
    let agent = host_source(&read(AGENT), "agent");
    let mut previous = None;
    for step in [
        "uses: ./.agentic-job-source/prepare",
        "run: python3 .agentic-job-source/workflow/runner-setup.py admit",
        "run: agentic-job secure-host inference .agentic-job/config.toml",
        "install -D -m 0755 .agentic-job/agentic-job \"$RUNNER_TEMP/agentic-job-bin/agentic-job\"\n",
        "agentic-job config --host --prepared --from .agentic-job/config.toml --string \"host.env.PATH=$RUNNER_TEMP/agentic-job-bin:/usr/local/bin:/usr/bin:/bin\" > \"$RUNNER_TEMP/agentic-job-host.toml\"\n",
        "run: sudo /usr/local/bin/agentic-job sandbox prepare --config \"$RUNNER_TEMP/agentic-job-host.toml\"",
        "uses: ./.agentic-job-source/secure-host",
        "run: python3 .agentic-job-source/workflow/runner-setup.py setup",
        "agentic-job sandbox check --unless-passed",
        "if sudo -n true 2> /dev/null; then",
        "found=$(env -i PATH=\"$path\" sh -c 'command -v agentic-job')",
        "cmp \"$found\" .agentic-job/agentic-job ||",
        "uses: ./.agentic-job-source/run",
        "runner-home: ${{ runner.temp }}/agent-home\n",
        "github-token: ''\n",
    ] {
        let index = smoke.find(step).unwrap_or_else(|| panic!("missing {step}"));
        assert!(previous.is_none_or(|p| index > p), "{step}");
        previous = Some(index);
        if step.starts_with("uses: ") || step.starts_with("run: ") || step.contains("$RUNNER_TEMP")
        {
            assert!(agent.contains(step), "the agent job runs {step}");
        }
    }
    excludes(
        smoke,
        &[
            "continue-on-error:",
            "check: false",
            "check: 'false",
            "check: \"false",
            "secrets.",
            "/var/tmp/agentic-job",
            "sandbox.env",
            "inference-run-token",
        ],
    );
    // Read only, as the agent job: no identity token where the agent runs.
    assert_eq!(
        yaml(&text)["jobs"]["wrapper-runner"]["permissions"],
        yaml("{contents: read, actions: read}")
    );
    // The handback goes through the wrappers' check, and only there:
    // nothing applies or publishes it.
    let check = job(&text, "wrapper-check");
    includes(
        check,
        &[
            &format!("check.yml@{PIN}"),
            "policy: ${{ toJSON(needs.wrapper-policy.outputs) }}",
            "safe-outputs-artifact-id: ${{ needs.wrapper-runner.outputs.safe-outputs-artifact-id }}",
            "safe-outputs-artifact-digest: ${{ needs.wrapper-runner.outputs.safe-outputs-artifact-digest }}",
        ],
    );
    excludes(check, &["secrets"]);
    assert_eq!(
        yaml(&text)["jobs"]["wrapper-check"]["permissions"],
        yaml("{contents: read, actions: read, id-token: write}")
    );
    // Runs, and fails, when check was skipped for want of a handback.
    let verdict = job(&text, "wrapper-scripted-dispatch");
    includes(
        verdict,
        &[
            "needs: [wrapper-runner, wrapper-check]",
            "if: ${{ !cancelled() }}",
            "permissions: {}",
            "EXIT: ${{ needs.wrapper-runner.outputs.exit }}",
            "CHECKED: ${{ needs.wrapper-check.outputs.artifact-id }}",
            "[[ \"$EXIT\" == 0 && \"$CHECK\" == success && \"$CHECKED\" =~ ^[1-9][0-9]*$ && -z \"$REFUSAL\" && \"$PATCH\" != true ]]",
        ],
    );
    excludes(
        &text,
        &[
            "apply.yml",
            "SAFE_OUTPUTS_PAT",
            "pull-requests: write",
            "issues: write",
        ],
    );
}

#[test]
fn separate_immutable_host_pin_retains_wrapper_and_smoke_contracts() {
    let direct = format!("cgwalters-forge/actions/secure-host@{}", "a".repeat(40));
    assert!(!PIN.starts_with(&"a".repeat(40)));
    for (file, name) in [(AGENT, "agent"), (SMOKE, "wrapper-runner")] {
        let text = read(file);
        // Also works after production has moved from the forwarder to a direct pin.
        let local = host_source(&text, name);
        let fixture = local.replace(LOCAL_HOST, &direct);
        if name == "agent" {
            agent_source(&fixture);
            agent_inputs(&fixture);
        } else {
            smoke_source(&fixture);
        }
        for invalid in [
            "cgwalters-forge/actions/secure-host@main".to_owned(),
            "cgwalters-forge/actions/secure-host@v0.6.9".to_owned(),
            format!("cgwalters-forge/actions/secure-host@{}", "a".repeat(39)),
            format!("cgwalters-forge/actions/secure-host@{}g", "a".repeat(39)),
            format!(
                "cgwalters-forge/actions/secure-host/extra@{}",
                "a".repeat(40)
            ),
            format!("other/actions/secure-host@{}", "a".repeat(40)),
            format!("{LOCAL_HOST}/extra"),
        ] {
            let changed = local.replace(LOCAL_HOST, &invalid);
            assert!(std::panic::catch_unwind(|| host_source(&changed, name)).is_err());
        }
    }
    for file in [DISPATCH, REVIEW] {
        let text = read(file);
        wrapper_release_and_order(&text);
        if file == REVIEW {
            review_source(&text);
        }
        for call in ["policy", "check", "apply"] {
            let changed = text.replace(
                &format!("/{call}.yml@{PIN}"),
                &format!("/{call}.yml@{} # v0.6.9", "b".repeat(40)),
            );
            assert!(std::panic::catch_unwind(|| wrapper_release_and_order(&changed)).is_err());
        }
    }
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
fn real_reviews_are_opt_in_and_use_only_checked_artifact_at_admitted_head() {
    let text = read(REVIEW);
    let source = yaml(&text);
    let jobs = &source["jobs"];
    // Advisory policy suppresses the verdict. The ordinary publishers must
    // take the admitted head and checked verdict from the main pipeline.
    assert!(jobs["policy"]["with"]["advisory"].is_badvalue());
    for (name, dependencies, command) in [
        ("submit", "[policy, agent, check]", "review submit"),
        (
            "status",
            "[policy, agent, check, submit, apply]",
            "review status",
        ),
    ] {
        let publisher = &jobs[name];
        assert_eq!(publisher["needs"], yaml(dependencies), "{name}");
        let step = publisher["steps"]
            .as_vec()
            .unwrap()
            .iter()
            .find(|step| {
                step["run"]
                    .as_str()
                    .is_some_and(|run| run.contains(command))
            })
            .expect("review publishing step");
        for (key, expected) in [
            ("HEAD_SHA", "${{ needs.policy.outputs.review-head }}"),
            ("VERDICT", "${{ needs.check.outputs.verdict }}"),
        ] {
            assert_eq!(step["env"][key].as_str(), Some(expected), "{name}: {key}");
        }
    }
    includes(
        job(&text, "submit"),
        &[
            "vars.AGENT_REAL_REVIEWS == 'true'",
            "needs.check.result == 'success' && needs.check.outputs.artifact-id != ''",
            "artifact-ids: ${{ needs.check.outputs.artifact-id }}",
            "review submit --accepted \"$RUNNER_TEMP/checked/agent_output.json\" --output \"$GITHUB_OUTPUT\"",
            "review-url: ${{ steps.submit.outputs.review-url }}",
        ],
    );
    let apply = jobs["apply"]["if"].as_str().unwrap();
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
    for body in [
        job(&review, "refusal"),
        job(&review, "submit"),
        job(&review, "status"),
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
    includes(
        job(&review, "refusal"),
        &["\"$RUNNER_TEMP/bin/agentic-job\" status"],
    );
}

#[test]
fn prose_is_separate_default_off_and_uses_exact_advisory_contract() {
    let text = read(REVIEW);
    let source = yaml(&text);
    let jobs = &source["jobs"];
    for main in [
        "admit", "policy", "refusal", "register", "agent", "check", "submit", "apply", "status",
    ] {
        excludes(
            job(&text, main),
            &["needs.prose-", "prose-pull", "REVIEW_PROSE"],
        );
    }
    includes(
        job(&text, "prose-config"),
        &[
            "needs: policy",
            "needs.policy.outputs.admitted == 'true'",
            "REVIEW_PROSE: ${{ vars.REVIEW_PROSE }}",
        ],
    );
    includes(
        job(&text, "prose-policy"),
        &[
            "needs.policy.outputs.admitted == 'true'",
            "needs.prose-config.outputs.enabled == 'true'",
        ],
    );
    let with = &jobs["prose-policy"]["with"];
    for (key, value) in [
        ("advisory", true),
        ("event", true),
        ("review", true),
        ("apply-partial", false),
    ] {
        assert_eq!(with[key], Yaml::Boolean(value), "{key}");
    }
    for (key, value) in [
        ("kind", "analysis"),
        ("outputs", "add_comment,noop"),
        ("max-outputs", "1"),
        ("notify", "none"),
        ("task", ""),
        ("review-task", "workflow/prose.md"),
        ("artifact-prefix", "prose-"),
    ] {
        assert_eq!(with[key].as_str(), Some(value), "{key}");
    }
    // The policy id does not namespace uploads. Both policy calls run in the
    // same workflow: the prefix must separate every policy/run/check artifact.
    assert!(jobs["policy"]["with"]["artifact-prefix"].is_badvalue());
    for suffix in [
        "agentic-job-binary",
        "agentic-job-policy",
        "agent-run",
        "agent-transcript",
        "safe-outputs",
        "checked-outputs",
        "applied",
    ] {
        assert_ne!(suffix, format!("prose-{suffix}"));
    }
    for key in [
        "review-item",
        "review-head",
        "comment-target",
        "output-repo",
        "item-repo",
        "apply-environment",
        "proposals-artifact",
    ] {
        assert!(with[key].is_badvalue(), "advisory forbids {key}");
    }
    for name in ["prose-policy", "prose-check", "prose-apply"] {
        assert!(
            jobs[name]["uses"]
                .as_str()
                .unwrap()
                .ends_with(PROSE_PIN.split_once(' ').unwrap().0)
        );
        excludes(
            job(&text, name),
            &[
                "SAFE_OUTPUTS_PAT",
                "statuses: write",
                "classifier",
                "verdict",
            ],
        );
    }
    includes(
        job(&text, "prose-check"),
        &[
            "safe-outputs-artifact-id: ${{ needs.prose-agent.outputs.safe-outputs-artifact-id }}",
            "safe-outputs-artifact-digest: ${{ needs.prose-agent.outputs.safe-outputs-artifact-digest }}",
        ],
    );
    includes(
        job(&text, "prose-bounds"),
        &[
            "needs.prose-check.outputs.artifact-id != ''",
            "needs.prose-check.outputs.artifact-digest",
            "digest-mismatch: error",
            "node workflow/prose.mjs check",
        ],
    );
    includes(
        job(&text, "prose-apply"),
        &[
            "needs.prose-agent.outputs.exit == '0'",
            "needs.prose-check.result == 'success'",
            "needs.prose-bounds.result == 'success'",
            "cancel-in-progress: false",
            "check: ${{ toJSON(needs.prose-check) }}",
        ],
    );
}

#[test]
fn prose_runner_only_migration_keeps_oidc_and_writes_out_of_agent_job() {
    let shared = read(AGENT);
    let text = job(&shared, "agent");
    excludes(
        &text,
        &[
            "id-token:",
            "agent-user: sandbox",
            "pull-requests: write",
            "INFERENCE_API_KEY",
        ],
    );
    includes(
        &text,
        &[
            "runner-home:",
            "AGENT_USER: runner",
            "runner-setup.py admit",
            "runner-setup.py setup",
            "inference-run-token: ${{ secrets.INFERENCE_RUN_TOKEN }}",
            // The agent's `agentic-job handback` must find the policy
            // binary, not the older one secure-host's image installs.
            "install -D -m 0755 .agentic-job/agentic-job \"$RUNNER_TEMP/agentic-job-bin/agentic-job\"\n",
            "agentic-job config --host --prepared --from .agentic-job/config.toml --string \"host.env.PATH=$RUNNER_TEMP/agentic-job-bin:/usr/local/bin:/usr/bin:/bin\" > \"$RUNNER_TEMP/agentic-job-host.toml\"\n",
        ],
    );
    let source = yaml(&shared);
    assert!(source["jobs"]["agent"]["if"].is_badvalue());
    assert!(source["jobs"]["prose"].is_badvalue());
    assert_eq!(
        source["on"]["workflow_call"]["inputs"]["prose"]["default"],
        Yaml::Boolean(false)
    );
    assert_eq!(
        source["jobs"]["agent"]["permissions"],
        yaml("contents: read\nissues: read\npull-requests: read\nactions: read")
    );
    let steps = source["jobs"]["agent"]["steps"].as_vec().unwrap();
    for output in [
        "exit",
        "safe-outputs-artifact-id",
        "safe-outputs-artifact-digest",
    ] {
        assert_eq!(
            source["on"]["workflow_call"]["outputs"][output]["value"].as_str(),
            Some(format!("${{{{ jobs.agent.outputs.{output} }}}}").as_str())
        );
    }
    let index = |part: &str| {
        steps
            .iter()
            .position(|step| format!("{step:?}").contains(part))
            .unwrap()
    };
    assert!(index("prepare") < index("runner-setup.py admit"));
    assert!(index("runner-setup.py admit") < index("config --host"));
    assert!(index("config --host") < index("cgwalters-forge/actions/secure-host"));
    assert!(index("cgwalters-forge/actions/secure-host") < index("runner-setup.py setup"));
    assert!(index("runner-setup.py setup") < index("node workflow/prose.mjs"));
    assert_eq!(
        steps[index("node workflow/prose.mjs")]["if"].as_str(),
        Some("inputs.prose")
    );
    assert_eq!(
        steps.last().unwrap()["if"].as_str(),
        Some("${{ !inputs.prose || steps.prose.outputs.ready == 'true' }}")
    );
    assert_eq!(steps.last().unwrap()["id"].as_str(), Some("run"));
    let review = read(REVIEW);
    includes(
        job(&review, "prose-agent"),
        &[
            "uses: ./.github/workflows/cgwalters-agent.yml",
            "prose: true",
            "github-reads: true",
            "TAILNET_KEY: ${{ needs.prose-register.outputs.tailnet-key }}",
            "tailnet-key-attempt: ${{ needs.prose-register.outputs.tailnet-key-attempt }}",
            "tailnet-key-minted-at: ${{ needs.prose-register.outputs.tailnet-key-minted-at }}",
        ],
    );
    includes(
        job(&review, "prose-register"),
        &[
            "id-token: write",
            "register --config",
            "mint: 'true'",
            "Refuse unsupported child agents before credentials",
        ],
    );
    excludes(
        job(&review, "prose-agent"),
        &["id-token:", "secrets: inherit"],
    );
}

#[test]
fn prose_offline_fixtures_run_in_existing_wrapper_suite() {
    let out = Command::new("node")
        .args(["--test", "workflow/prose.test.mjs"])
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
fn disabled_legacy_callers_need_no_prose_files_and_detection_is_not_bypassed() {
    let workflow = yaml(&read(REVIEW));
    let steps = workflow["jobs"]["prose-config"]["steps"].as_vec().unwrap();
    let script = steps.last().unwrap()["run"].as_str().unwrap();
    assert_eq!(steps.last().unwrap()["shell"].as_str(), Some("node {0}"));
    // Execute the real inline config gate in an empty legacy caller checkout.
    // No helper, task, or inference endpoint exists there.
    for (raw, detector, reason) in [
        ("", "", "disabled"),
        (r#"{"enabled":false}"#, "", "disabled"),
        ("malformed", "", "invalid"),
        (r#"{"enabled":true}"#, "", "missing trusted prose files"),
        (
            r#"{"enabled":true}"#,
            "required-detector",
            "no authorized advisory allocation",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let summary = temp.path().join("summary");
        let output = temp.path().join("output");
        let result = Command::new("node")
            .args(["-e", script])
            .current_dir(temp.path())
            .env("REVIEW_PROSE", raw)
            .env("DETECT_MODEL", detector)
            .env("GITHUB_STEP_SUMMARY", &summary)
            .env("GITHUB_OUTPUT", &output)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert!(fs::read_to_string(summary).unwrap().contains(reason));
        assert!(!output.exists(), "must not enable a skipped chain");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("workflow")).unwrap();
    fs::write(
        temp.path().join("workflow/prose.mjs"),
        read("workflow/prose.mjs"),
    )
    .unwrap();
    fs::write(
        temp.path().join("workflow/prose.md"),
        read("workflow/prose.md"),
    )
    .unwrap();
    let output = temp.path().join("output");
    let result = Command::new("node")
        .args(["-e", script])
        .current_dir(temp.path())
        .env("REVIEW_PROSE", r#"{"enabled":true,"model":"provider/cheap","runner":"[\"ubuntu-26.04\"]","requests":5,"minutes":2,"usd":0.05,"verified":true,"evidence":"https://example.com/verification","inputPrice":1,"outputPrice":2,"mainInputPrice":10,"mainOutputPrice":20}"#)
        .env("DETECT_MODEL", "")
        .env("INFERENCE_URL", "https://example.com/inference")
        .env("GITHUB_STEP_SUMMARY", temp.path().join("summary"))
        .env("GITHUB_OUTPUT", &output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(
        fs::read_to_string(output)
            .unwrap()
            .contains("enabled=true\n")
    );
    // No override reaches upstream check: its normal required validation stays.
    let with = workflow["jobs"]["prose-check"]["with"].as_hash().unwrap();
    assert_eq!(with.len(), 3);
    assert_eq!(
        steps.last().unwrap()["env"]["DETECT_MODEL"].as_str(),
        Some("${{ vars.AGENTIC_JOB_DETECT_MODEL }}")
    );
}

#[test]
fn prose_register_preflight_is_the_tested_main_preflight_before_credentials() {
    let workflow = yaml(&read(REVIEW));
    let steps = workflow["jobs"]["prose-register"]["steps"]
        .as_vec()
        .unwrap();
    let main = workflow["jobs"]["register"]["steps"].as_vec().unwrap();
    let preflight = |step: &&Yaml| {
        step["name"].as_str() == Some("Refuse unsupported child agents before credentials")
    };
    let position = steps.iter().position(|step| preflight(&step)).unwrap();
    assert_eq!(
        steps[position]["run"],
        main.iter().find(preflight).unwrap()["run"]
    );
    assert!(
        steps[..position]
            .iter()
            .all(|step| step["id"].as_str() != Some("tailnet")
                && step["id"].as_str() != Some("register"))
    );
    assert!(
        steps[0]["run"]
            .as_str()
            .unwrap()
            .contains(PIN.split_once(' ').unwrap().0)
    );
    assert_eq!(
        steps[1]["with"]["artifact-ids"].as_str(),
        Some(
            "${{ needs.prose-policy.outputs.binary-artifact-id }},${{ needs.prose-policy.outputs.policy-artifact-id }}"
        )
    );
}
