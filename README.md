# actions

> This text is LLM generated.

GitHub Actions for cgwalters-forge and bootc-dev workflows, with one action per
directory. Use them as `cgwalters-forge/actions/<name>@main`. Work is tracked
in this repository's issues.

| Action | Status | Roadmap |
| --- | --- | --- |
| [lint](lint/README.md) | actionlint, zizmor and the runner-label policy; this repository's CI calls it | Called from each forge repository's CI |
| [workflows-check](lint/workflows-check.md) | Required workflow over a pull request's workflow files: job shape, new dangerous triggers, new zizmor ignores, linter configuration and `.gitignore` changes, checker references; CI runs its tests with fixture workflows | An organization ruleset requires it, evaluate mode first |
| [secure-host](secure-host/README.md) | Runs agentic-job's attested, digest-pinned image: no later step has root or the metadata service, world-write by owner only; CI runs the two-job example, the inputs and the world-write policy on ubuntu-26.04 | agentic-job's own action forwards here; opt-in egress (agentic-job#995) |
| [app-token](app-token/README.md) | Mints and masks one role's GitHub App installation token, scoped to named repositories; `remint.mjs` mints again near expiry or after a 401; CI runs its unit tests, no real mint (no app here) | Move each role to its app (tracker#766) |
| [admit](admit/README.md) | Fails unless the calling repository's owner id is in its owners.json; both reusable wrappers run it first; CI runs its unit tests | Brokers and tailnet federation match the same ids |
| [deployment](deployment/README.md) | Outputs the deployment's defaults from its deployment.json (the agent configuration repository and model); the wrappers use them after their input and variable; CI runs its unit tests | More defaults that name our resources move there |
| [renovate](renovate/README.md) | A Renovate preset that moves GitHub-hosted Ubuntu runner labels and nothing else; this repository extends it; `just test-renovate` validates it and runs the opt-in Rust dry-run harness against pinned Renovate, not in CI | Each forge repository extends it, once the Renovate app is installed for it |
| [tailscale](tailscale/README.md) | Settings by organization; CI runs its unit tests and checks the socket and CLI are closed after a join that fails, no real join (no tailnet credentials here) | Replace devspace.yml's and agent.yml's tailscale/github-action steps |

## cgwalters-agentic-job

One reusable workflow is here too:
[cgwalters-agentic-job](.github/workflows/cgwalters-agentic-job.yml), an
agent run of cgwalters-forge/agentic-job with this deployment's tailnet
join, inference proxy, runner, agent and model. Our repositories call it
at main. Any public repository can call it too, and its run's identity
tokens carry the same `job_workflow_ref`, so the inference broker and the
tailnet federation must match `repository_owner_id` against the ids in
[admit/owners.json](admit/owners.json) as well as that `job_workflow_ref`;
both wrappers reject any other owner before assigning the job to the
caller-selected runner ([admit](admit/README.md)). With an
`inference-url` other than ours it needs an `inference-audience` too, so
that our broker's audience is not sent elsewhere by default (the review
wrapper's variables still default apart). It refuses runs
from a private repository, so agentic-job's opt-in for private
repositories stays off. That opt-in admits only the calling repository.

A repository gets agent runs with the workflow below, a bounds file at
`.github/agentic-job/dispatch.toml` (and `fix.toml` for the `fix` kind), a
runner configuration at `.github/agentic-job/hosted.toml`, and an
environment that gates the apply token; cgwalters-forge/tracker has all four
to copy. The file's header says what else a caller may set.

For an issue tracked outside the work repository, pass optional `item-repo`
as `OWNER/NAME`. Empty keeps the issue in `repo`; `repo` and `output-repo`
still select the work and PR target. Issue lookup, status comments, steering
and hand-back references use `item-repo`. The pinned policy requires it to
be public and within the caller's `repos` bounds, and check enforces those
bounds so an agent cannot redirect privileged comments to an unapproved
repository. It is not supported for `review` or `fix`.

Before adopting v0.6.5, callers must migrate every bounds file (`allow`,
`fix-allow` and both review bounds): move `[outputs]`, `[trigger]`,
`[memory]` and `[secondary_pull_request]`, when present, under `[github]`
(for example, `[github.outputs]`). Keep shared limits and repository
selectors at the top level; exactly one forge section is required.
Policy calls also require `actions: read` in the caller's permissions.

Both wrappers pin agentic-job release v0.6.13 by commit (`4439980`), with
the version beside the pin so the release train can find it. It carries the
outside-host control probe retry (agentic-job#3227) and the `own_author`
push bound (agentic-job#3164), which tracker's fix runs need for pull
requests the bot authored.
This does not update the installed secure-host image or prove the observed
network-control failures fixed (agentic-job#3225). Image adoption and a
fresh-host canary remain separate work (agentic-job#2070/#2656).

Since v0.6.12 the agent is the runner's user, the only agent user
(agentic-job#2989), and the wrappers follow upstream's runner-only
boundary. The agent can read whatever its job holds, so the agent job,
[cgwalters-agent.yml](.github/workflows/cgwalters-agent.yml), has no
`id-token` and no repository secret: only read permissions, the run token
and a spent tailnet key. Before it, each wrapper's register job, the one
job with an identity token, runs on a hosted runner where nothing of the
agent's or the target's runs. It stays in the wrapper's own file because
the token's `job_workflow_ref` names the file that defines the job, and
that is what the broker and the tailnet admit. It joins the tailnet, mints
a single-use ephemeral key for the agent job's join
([tailscale](tailscale/README.md#a-job-without-an-identity-token)), and
registers the run at the broker with the policy job's binary. Both values
reach the agent job only as secrets of its call, which masks them in its
log; the agent job is called by local path, so at the wrapper's commit.
Re-running only a failed agent job can reuse a spent key, and an agent job
can wait past its key's hour; both fail at a check before the join that
says so. Use **Re-run all jobs**, not **Re-run failed jobs** (see
[retry recovery](#retry-recovery)).
The agent job then runs upstream's agent.yml sequence: prepare,
agentic-job's runner-user admission, the join, the reachability check,
`agentic-job config --host` (with no `setup.script`), the pinned
`secure-host` forwarder, the setup script as the runner's user, and `run`
with `--runner-home`. The pinned image installs its own, older
`agentic-job` in `/usr/local/bin`, so the host configuration puts a copy
of the policy binary first on the agent's `PATH`: the agent's
`agentic-job handback` commands must match the binary `run` starts. The
`wrapper-runner` CI job in
[secure-host.yml](.github/workflows/secure-host.yml) runs that sequence on
a fresh runner, with no inference or tailnet, and checks that the host
passes run's own probe, the runner has lost sudo and the agent's `PATH`
finds the policy binary. It is also a deterministic scripted dispatch: the
policy call selects the pinned helper's scripted agent (`fake`, comment
profile), `run` starts it with an empty GitHub token and no run token, the
pinned `check.yml` checks its handback, and `wrapper-scripted-dispatch`
passes only for an agent exit of 0 and one checked comment with no patch.
Nothing is applied, and the job holds no secret. This is the
repository's own base configuration; the wrappers still refuse `fake` from
callers. It covers the shared agent job's steps and check's input, not the
tailnet join, registration or apply, which only real runs exercise.

The dispatch wrapper refuses these inputs before anything is provisioned:
`session` (and `idle-minutes`), `inference-register:
api-key`, `inference-provider` and an `INFERENCE_API_KEY`. The inputs stay
declared so a caller passing them gets that message. The scripted agent
`fake` stays unsupported. Upstream's own admission refuses a configuration
with `[mcp]`, `api-key` or a session listener. ACP's `[child-agent]`
(`acp-experiment`) is also unsupported under the pinned runner-only
implementation. To avoid joining the tailnet, minting a key and registering
inference for a run that `run` will refuse, both register jobs first parse
only the policy-composed `config.toml`, downloaded by policy's validated
artifact IDs after the existing source/provenance checks. Missing, empty,
malformed or over-1-MiB configuration fails closed, as does any top-level
`child-agent` key, including quoted, dotted and inline TOML forms. There is
no opt-in, automatic child removal or fallback experiment: remove the child
configuration explicitly or wait for supported ACP.

The early ACP refusal slice of #219 landed in #227. Session work is separate
in #225; this pin still refuses session listeners, and this change enables
neither sessions nor a second user. #220 added an actionable pre-join
refusal for spent/expired keys with the re-run-all-jobs mitigation, not fresh
keys per attempt. Queue expiry is still unfixed: a review queued for over
an hour can hold an expired key, now refused before the join. KVM access
and self-pin policy remain separate #219 checklist items too.

### Retry recovery

The supported retry contract is to **re-run all jobs**, so policy and the
trusted register job run again and issue a new single-use tailnet key. Do
not re-run only the agent job or use **Re-run failed jobs**: GitHub can retain
the successful register job's outputs from the previous attempt. Even if the
earlier agent failed before joining and never spent its key, that key still
expires one hour after minting.

A `/review` also acquires its key before waiting for per-PR agent concurrency.
If that wait exceeds an hour, let the older review finish, then re-run all
jobs for the failed review. A fresh authorized `/review` after the older run
finishes is another way to start with new registration. Runner-capacity waits
can also exhaust the hour; neither recovery guarantees a join if the new run
waits that long again. Avoid repeatedly retrying while the queue is blocked.

For #219, retain this recovery contract rather than grant a broker new
tailnet-key issuance authority. This does not refresh credentials at actual
agent start or fix long queues. Keep OIDC and standing mint credentials out
of the agent job; do not make keys reusable or extend their lifetime as a
workaround. #220's pre-join check only makes the failure clear.
Fresh-attempt issuance, live delayed-start/rerun acceptance, RHEL KVM
verification and self-pin lint policy remain outstanding.

### Session limitations

Session listener support requires a compatible agentic-job pin first: the
current pin rejects `session.listen` in both runner admission and `run`.
Keep the wrapper's early refusal until that dependency is updated, so a
listener request cannot register an inference run or mint a tailnet key
only to fail in the agent job.

`kind: session` is not that listener. It is a repository-triage run
(tracker#1505): one pass over the target repository, started from an
issue (not a pull request), that runs once and accepts no attach.
The wrapper sends it through the triage path unchanged: policy's
`analysis` kind, so the tree stays clean and no branch is pushed;
`add_comment`, `noop`, `missing_tool` and `missing_data` against the
caller's `allow` file, at most 3 outputs; the checked apply; and the
caller's `budget` and `max-requests` (defaults 500 AIC and 400 requests,
requests at most 1000), the 75-minute timeout and the broker's aggregate
limits. A session needs no other
caller setting than adding `session` to its `kind` choices, and the
`session` boolean stays refused with it.

A triage hands back only those four outputs unless its caller sets
`triage-outputs-from-allow: true` (off by default, and refused for any
other kind). Then the wrapper asks policy for `all`, so the output types
are the ones the `allow` file's `[github.outputs]` grants, never a new
pull request or a push for this `analysis` kind, and still at most 3.
A caller sets it only together with an `allow` file of its own for the
route, such as PR recovery's, which must close an obsolete bot pull
request or file a replacement issue; that file lists `noop`,
`missing_tool` and `missing_data` too if the run may use them.

The release train may replace only the local `secure-host` forwarder in the
agent job and the wrapper smoke job with
`cgwalters-forge/actions/secure-host@<full immutable commit>`: the verified
image PR's merge commit, independently of the agentic-job release commit.
Mutable refs and other action sources are not accepted by the contract tests.
Policy, check, apply and the policy-source checkout/prepare/run sequence stay
on the agentic-job pin.

Since v0.6.0 the agent's home no longer copies the
runner image's `/etc/skel`, including toolchains and dotfiles.
Mid-run steering stays off unless the caller's runner configuration enables
`[session] watch-item = true`. Policy derives the watched item from the
dispatch wrapper's `issue` (in `item-repo`, or `output-repo` by default) or the review event;
`run` doesn't need any additional input. Upstream limits delivery to 10 updates of
8000 characters, quoted as untrusted item text, not instructions.

```yaml
name: dispatch
run-name: ${{ inputs.kind }} ${{ inputs.repo }}#${{ inputs.item }}
on:
  workflow_dispatch:
    inputs:
      task: {type: string, required: true}
      repo: {type: string, required: true}
      item: {type: string, required: true}
      kind: {type: choice, required: true, options: [implement, triage, research, session, review, fix]}
permissions: {}
concurrency:
  group: dispatch-${{ inputs.kind }}-${{ inputs.repo }}-${{ inputs.item }}
jobs:
  run:
    uses: cgwalters-forge/actions/.github/workflows/cgwalters-agentic-job.yml@main
    permissions: {contents: read, issues: write, pull-requests: write, actions: read, id-token: write}
    with:
      task: ${{ inputs.task }}
      repo: ${{ inputs.repo }}
      item: ${{ inputs.item }}
      kind: ${{ inputs.kind }}
      apply-environment: agent-apply
    secrets:
      SAFE_OUTPUTS_PAT: ${{ secrets.CGWALTERS_BOT_PAT }}
```

### Artifact-only research (dormant)

The concrete threat is posting attacker-controlled supervisor requests as the
bot before the trusted executor admits them. The caller-owned boolean
`research-artifact-only` defaults to **false** and is accepted only with
`kind: research`. It rejects implement, triage, fix, review and repository
sessions; the `session` listener and `idle-minutes` remain refused too.
The existing pinned policy/run/check path still enforces the caller's bounds,
analysis-only tree and three-output ceiling. No supervisor decoder, admission,
cap or ledger logic moves here.

When opted in, **apply is never called**, even with an empty PAT (which ordinary
apply would replace with the job token). No proposed effect is applied, including
comments, missing-data/tool reports and noops. The apply environment is not
required, and the PAT is not forwarded to policy. Policy notification is `none`:
there is no start or final status comment or reaction. A fixed, bounded job
summary and `research-status` report `ready` or `unavailable`, never agent prose.
Ordinary calls and both review paths keep their existing behavior.

Only successful target, admitted policy, successful agent with exit 0 and no
reported early stop, and successful check with a checked artifact and no patch
release identity outputs. Failed, missing or partial results are unavailable;
budget/timeout hand-backs are not accepted in this mode. Cancellation may skip
the reporting job and leave status empty. Treat empty status as unavailable too.
An agent can misreport completion; `ready` is transport/check status, not a
supervisor authorization or proof that its claims are true.

For `ready`, `safe-outputs-artifact-id` and `safe-outputs-artifact-digest` name
the **original untrusted upload**, not check's sanitized artifact. The digest is
64 lowercase hex characters. `handback-repository`, `handback-run-id`,
`handback-run-attempt` and `handback-agent-job` identify the current reporting
attempt and producer (the last is the agent job's check run ID). No payload body
is a workflow output. A trusted
caller must bind these to its own expected repository/run/latest attempt and
reviewed workflow revision, confirm the producer in that attempt's jobs API,
and fetch artifact metadata **by ID**, requiring the same workflow run and
`sha256:<digest>`. Download that ID with digest-mismatch failure, never by an
unchecked artifact name. Partial reruns can retain an earlier attempt's agent
outputs: reject them when the producer is absent from the reporting attempt,
even if status is `ready`; start a new dispatch rather than consuming them.
The archive's `outputs.jsonl` is attacker-controlled
data, not shell, a configuration source or direct GitHub write authority.
The pinned check already verifies the original ID/digest and same-run ownership;
the caller's provenance checks must not substitute an artifact from another run.

Tracker migration and activation remain outstanding (tracker#939). Its trusted
`.github/workflows/supervisor.yml` caller must opt in only after downloading
same-run requests for `supervise execute --requests FILE --ledger-out LEDGER`.
Follow [agent-board's hand-back contract](https://github.com/cgwalters-forge/agent-board/blob/main/docs/supervisor-requests.md#hand-back):
authenticate the newest prior executor ledger and its run, keep executor and
agent artifacts separate, serialize executors, fail closed on missing or invalid
ledger continuity, and persist reservations/ledger even after write failures.
Prove that continuity before activation; merging this wrapper does not satisfy
the deployment gate. For rollback to comment/result-url delivery, keep write
caps at zero for a day to avoid undercounting the ledger's writes. This change
enables no caller, settings, credentials, admission or live dispatch.

### Secondary pull requests

The wrappers need no secondary-specific input. Policy, check and apply all
pin agentic-job v0.6.15 (`489c69d`); its
[workflow contract](https://github.com/cgwalters-forge/agentic-job/blob/740ff7ff4dcef59dcb0619eb14d0e357c3193f62/docs/workflow.md)
and [safe-output contract](https://github.com/cgwalters-forge/agentic-job/blob/740ff7ff4dcef59dcb0619eb14d0e357c3193f62/docs/safe-outputs.md)
govern this feature, not the older v0.6.11 release plan.

The concrete threat is an agent redirecting a privileged write to another
repository. Secondary publication stays off unless the caller's bounds file
sets `[github.secondary_pull_request]`, with an exact `repo`, `base`,
`branch_prefix`, `max = 1`, `max_patch_bytes` and `max_patch_files`.
Check enforces these bounds independently; apply consumes only checked outputs.
Secondary file exemptions must be explicit in that table's
`unprotected_files`; primary exemptions are not inherited. See the pinned
contract for the patch/base metadata and run-specific branch requirements.

Dispatch forwards the caller's `allow` (or `fix-allow` for fixes) to policy,
with a total `max-outputs` of **3**, including any secondary request. Both
review paths allow only **1** output. The caller's `max_outputs` must admit
the requested total; per-type limits still apply. A secondary request adds
one to the primary `create_pull_request.max` collector allowance, not to the
wrapper's total ceiling: two PRs leave room for only one other request.
Do not raise a caller's total above 3 expecting this wrapper to request more.
Tracker's activation child owns its bounds changes and the live
primary-plus-secondary publication test; this audit enables no caller.

Both wrappers explicitly forward `SAFE_OUTPUTS_PAT` to apply, never to the
agent or check. Secondary publication requires that PAT, with access to fork
the secondary repository, disable Actions on its fork, push there and open a
PR; the job token is insufficient. The bot must not own the secondary target.

Live acceptance of the migrated wrappers is recorded by tracker dispatch
[42348718765](https://github.com/cgwalters-forge/tracker/actions/runs/42348718765)
(register, agent, check and apply succeeded) and event review
[42320676566](https://github.com/cgwalters-forge/actions/actions/runs/42320676566)
(register, agent, check and submit succeeded). Both runs' `referenced_workflows`
resolve the wrappers and shared agent job to `ba6c569`, and the helpers to
`427ce22`. The older misctool review `41920939510` resolves to wrapper
`5906e674` and helpers v0.6.1, so it is not acceptance of this migration.
These are real agent runs, not the host smoke test; they do not prove
primary-plus-secondary publication or complete #138's required canary rollout.

Two later runs record the runner-only boundary at each wrapper revision
(actions#103). Tracker dispatch
[42320486895](https://github.com/cgwalters-forge/tracker/actions/runs/42320486895)
(research on actions#196): its `run` job resolves the dispatch wrapper and
shared agent job to `ba6c569` and policy, check and apply to `427ce22`.
The `3f9f642` helpers that `referenced_workflows` also lists belong to
tracker's skipped `review` job, pinned to wrapper `f05e237`. Register, agent,
check and apply all succeeded, and apply posted the checked comment
[6699900595](https://github.com/cgwalters-forge/actions/issues/196#issuecomment-6699900595),
showing the agent as `runner` running the policy binary 0.6.12.
Event review
[42376903094](https://github.com/cgwalters-forge/actions/actions/runs/42376903094)
of #223 resolves the review wrapper and agent job to `abc3dba` and the
helpers to `427ce22`. Its submit job posted APPROVED review
[5812467802](https://github.com/cgwalters-forge/actions/pull/223#pullrequestreview-5812467802)
with `commit_id` `7a3ffdd`, the admitted head, and apply was skipped. In
both, the agent job's token had only Actions, Contents, Issues,
PullRequests and Metadata read and no `id-token`. Both checkouts set
`persist-credentials: false`, and only the register job minted the tailnet
key, "a single-use, ephemeral key, valid for 60 minutes", which the agent
job's join spent. Between `ba6c569` and `e084c52` the wrappers changed
only their `# main` self-pins. Those pins moved from `1e867fc`, and across
that range `admit`, `deployment` and `tailscale` changed only in
`tailscale/setup.test.mjs`. A real dispatch and review at a
later wrapper revision with other changes still need recording again.

## Review

[cgwalters-review](.github/workflows/cgwalters-review.yml) is the reusable
event-review wrapper. It composes agentic-job's policy, check, apply and
review helpers at the pinned commit, with the dispatch wrapper's register
and agent jobs, so that the tailnet and the inference broker admit this
file's `job_workflow_ref`. agentic-job's review workflow can now run a
caller's own network step instead (its #705), but callers have not switched
to it. It refuses private repositories, as the dispatch wrapper does. It adds
our tailnet join and defaults: opencode, the model from
[deployment.json](deployment/README.md), `ubuntu-26.04`, and the dispatch
wrapper's inference URL and audience. It has **no inputs**. Repository/organization variables
`AGENT`, `AGENT_MODEL`, `AGENT_RUNNER` (JSON), `INFERENCE_URL`,
`INFERENCE_AUDIENCE`, `AGENT_CONFIG`, `AGENT_CONFIG_REPO` and
`AGENT_CONFIG_PATH` select owner-controlled configuration. Set the last
two for agents needing external configuration; there is no hard-coded
configuration repository. `AGENT_CONFIG` otherwise uses the pinned
agentic-job hosted configuration. `AGENTIC_JOB_TAILSCALE_OAUTH_CLIENT_ID`
and `AGENTIC_JOB_TAILSCALE_TAGS`, read by both wrappers, are the tailnet
login of an organization that [tailnets.json](tailscale/tailnets.json)
doesn't list. When set, they also replace a listed organization's entry. Either
way the wrappers run only for an owner that
[admit/owners.json](admit/owners.json) lists.

Call it from the same `pull_request_target` and `issue_comment` events and
job condition as [this repository's caller](.github/workflows/review.yml),
replacing the job's `uses` with:

```yaml
uses: cgwalters-forge/actions/.github/workflows/cgwalters-review.yml@main
```

Keep that caller's six permission grants. Optional `SAFE_OUTPUTS_PAT` is
forwarded explicitly, only to apply; it never reaches the agent or the real
review submission. Keep `workflow/review.toml`, `workflow/review-command.toml`
and `workflow/review.md` on the caller's base branch. Automatic reviews admit
non-draft owner/member/collaborator PRs, preserving upstream's association
rule for all authors. Otherwise-unassociated bots wait unless `REVIEW_BOT_IDS`
lists their numeric IDs **and** the bounds admit their logins (including
Dependabot). Use a JSON array of numbers; Actions also coerces string IDs.
`/review` still requires write access through event policy.
Only admitted agent jobs enter per-PR concurrency; new automatic events
cancel the old agent, comments do not. The head is data inside the sandbox,
never executed by a privileged step. Each review is bounded to 150 requests,
500 AIC and 75 agent minutes.

The agent keeps the read-only job token; it has no organization Projects
credential. To avoid false missing-board blockers, `workflow/review.md`
requires visibility of the exact Project and a complete membership query
before reporting an item absent. Hidden, denied or ambiguous membership is
reported as "membership could not be verified", not as missing. Callers that
keep their own copy of the review task must copy this guidance too; changing
the reusable workflow alone does not update a caller's task.

`AGENT_REAL_REVIEWS=true` opts into checked APPROVE/REQUEST_CHANGES
submission with the job token, at the admitted head. Leaving it unset keeps this off. Enable
GitHub Actions' permission to create/approve PRs for this feature. A stale
head, cancellation or failed check cannot produce a new approval. The
compatibility `review` status is not a merge gate or human approval.

For a new repository in this organization, put the review caller and the
three `workflow/review*` files in its **initial commit**, and set the
variables above (copy them from an onboarded repository such as tracker)
and Actions' permission to approve pull requests when you create it. The
organization's ruleset requires an approving review on `main` and has no
bypass actors, but the first push to an empty repository isn't a pull
request, so the review is in place before the first one. If you add it later, the
onboarding pull request itself needs an approval from an organization
owner, because nothing can review it yet.

A repository whose name ends in `-sandbox` is for agents to iterate in
freely: the organization's approval ruleset excludes it, so its pull
requests merge without a review. When it's ready, renaming it to drop the
suffix brings it under the ruleset; the review above should be in place
before that rename.

Before migrating tracker (a separate repository), admit this wrapper's
`job_workflow_ref`, with the owner ids of admit/owners.json, in the
tailnet federation and inference broker, then
replace its review caller's `uses` as above and exercise a real review and
a synchronize cancellation. No ACL, OAuth or broker policy is changed here;
record those deployment changes in the private wiki when made. Removing the
deployment-specific pieces from agentic-job is a separate follow-up.
`just test-lint` checks the wrapper's admission and publication boundaries,
including the dispatched review's base-owned policy, checkout refs and
credential-free head access in Rust. Tests that execute the target and
submission JavaScript remain in the Node suite until those steps are ported;
live tailnet joins and GitHub review writes are not exercised locally.

The review wrapper pins this repository's `admit`, `deployment` and
`tailscale` actions by full commit too, so moving `main` cannot change
privileged code behind a caller's reviewed wrapper pin. Renovate's scoped
`git-refs` manager in [.github/renovate.json](.github/renovate.json) proposes
updates from `main`, grouped as `review actions` with automerge off. Unlike
release-tag updates, this works without this repository publishing tags. These
updates must merge through review here; a caller pinned to the wrapper's
commit gets them only by reviewing an update to that pin. The dispatch
wrapper continues to use these actions at `main` for non-review kinds;
its review kind uses separate literal commit-pinned steps.

### Advisory prose (default off)

The independent `prose-*` chain in the review wrapper uses agentic-job
**v0.6.15**, commit `740ff7ff4dcef59dcb0619eb14d0e357c3193f62`, for policy,
prepare/run, check and apply. Its policy binary, source actions and publishing
workflows must stay at that one commit. The main verdict composition retains
its independently matched pins and runner-user setup described above; the
advisory does not change its release or network configuration.
The advisory calls the same `cgwalters-agent.yml` runner-only path as the main
review, not another agent workflow or a second implementation. Its independent
register job validates policy provenance/configuration, joins and mints a
single-use ephemeral key; both that key and the capped run token reach the agent
as secrets. The shared runner checks attempt/expiry, spends the key, configures
`host.env.PATH`, hardens the host and runs setup unprivileged before bounded
prose preparation and run with `runner-home`. It has no OIDC, provider key, MCP
credential, write token or persistent network-join credential. No trusted-base
workflow exemption is added or workflow check weakened. The redundant
`cgwalters-prose-agent.yml` is already absent from main and stays absent.

The concrete threats are PR text obtaining publication authority, spending the
main review allocation, or attaching advice to an unreviewed head. The prose
chain therefore starts only after the existing authorized public-PR event
admission; it is analysis with head-bound advisory policy, one checked comment
or noop, no notification and no partial apply. Nothing in the main review
depends on prose results. It never submits reviews, sets statuses, classifies
PRs or escalates buckets. Advisory failure cannot change the main verdict.

`REVIEW_PROSE`, a trusted repository/organization variable containing JSON, is
the only opt-in. Unset or `{"enabled":false}` visibly skips in the prose-config
job summary. No cheap deployment has been selected or verified here. To enable,
the owner must supply **all** of these fields, not copy a fictional model/pool:
`enabled` (true), `model` (actual provider/model), `runner` (a JSON-encoded label
array for an isolated ephemeral runner using the shared single-use tailnet join),
`requests` (1–12), `minutes` (1–5), `usd` (0.01–0.10),
`verified` (true), `evidence` (HTTPS verification record), and `inputPrice`,
`outputPrice`, `mainInputPrice`, `mainOutputPrice` (positive prices in the same
units). Both prose prices must be strictly cheaper. The owner must verify the
model is actually served through the configured broker and record the date,
model identifiers and pricing evidence **before** setting `verified`. This is
an owner attestation, not a live availability/pricing probe. Supply
`INFERENCE_URL` from deployment configuration; there is no endpoint or main
reviewer fallback. Broker aggregate caps and run request/time/USD limits remain
necessary: reported spend may cross its cap on the last reply. No hard billing
guarantee is implied.

Only the trusted hosted registration job joins the tailnet with OIDC. Admit
this wrapper's registration `job_workflow_ref` in the broker and tailnet before
enabling. If no compatible runner or model exists, leave prose off; do not give
its agent OIDC or restore `agent-user: sandbox`.

The pinned checker has no per-call detector cap or skip input. If the trusted
`AGENTIC_JOB_DETECT_MODEL` variable is set, this optional **entire prose chain
skips visibly**, because that extra inference has no separately authorized
advisory allocation. The main review's required detector remains unchanged.
With it unset, upstream explicitly skips optional detection but still collects,
validates policy/output bounds and verifies artifacts before apply. Do not clear
a required detector just to enable prose; deploying both needs upstream bounded
detector composition first.

The base-owned `workflow/prose.md` and `workflow/prose.mjs` are required beside
the existing review bounds when enabled. Configuration checks disabled settings
before loading the helper; disabled legacy callers need neither prose file.
Missing enabled configuration/helper visibly skips. Input
preparation, after hardening, reads at most 100 changed-file
records and 100 commit records (2 MiB per API response, 30 seconds per call),
and supplies at most 20,000 Unicode characters: body up to 4,000, then commit
messages and changed Markdown hunks with remaining space. Hunk headers, paths,
commit SHAs and body/message line positions are retained; missing patches,
truncation and incomplete pagination are reported, not claimed as covered.
Changed Markdown, commit messages and body are serialized as untrusted JSON.
The checklist, manual prompt and constructed fixtures are fetched only from
tracker commit `5c4a8d61a94323703b71998c8b21eb7ecb2b386a` under
`docs/review-checklist/prose*.md`. Missing checklist/input, stale head and no
prose visibly skip without publishing. Instructions ask for exact before/after,
location and meaning-preservation rationale, or noop. The independent trusted
bounds job checks one to three structured rewrites and a 12,000-byte comment
after upstream checking; it passes upstream's checked artifact unchanged to
apply. This structural check is not proof of semantic correctness or immunity
to prompt injection. The pinned fixtures are reference material, not a model
evaluation result.

Policy, input preparation and apply bind the admitted head. Apply retains
same-run artifact ID/digest checks, deduplication and publisher-side live-head
rechecking, and adds trusted reviewed-SHA framing. GitHub has no conditional
issue-comment POST: a head can still move between the last read and POST. The
comment is framed as advice for the reviewed SHA, not guaranteed current advice.
Serialize publication; inspect the destination before retrying a lost POST
response. A later run can post another advisory. Body text is reviewed on
admitted runs; today's caller does **not** independently trigger on body-only
edits. No edited-body event is added by this change.

Roll out only after reviewing a full-SHA wrapper pin in each caller, copying the
two prose files to its trusted base and retaining its existing bounds and
permission grants. This repository's caller already uses the local reusable
wrapper, so its event configuration needs no new trigger or grant. For external
callers, update `.github/workflows/review.yml`'s `jobs.review.uses` to
`cgwalters-forge/actions/.github/workflows/cgwalters-review.yml@` followed by the
reviewed merge commit, not a moving tag. Wrapper merge alone activates nothing.
After cheap-model verification and credential-boundary checks, an authorized
owner must smoke-test an admitted **public** PR with a known prose rewrite,
commit-message-only/body cases and noop; confirm the exact reviewed SHA and one
advisory comment with no verdict/status effect. Also push a newer head before
publication, exercise missing config/artifacts and an advisory failure, and
confirm the main review result is unchanged. Disable by clearing `REVIEW_PROSE`.
Keep actions#195 open until deployment and live acceptance are recorded.

Offline tests cover input bounds, injection-as-data, missing configuration and
checklist, stale preparation heads, noop, structured output bounds, immutable
pins and isolation of the main dependency graph. They do not prove model
quality, live availability/pricing, broker admission, ephemeral-runner isolation
or live GitHub publication. Run `node --test workflow/prose.test.mjs` and
`just test-lint`. Set `AGENTIC_JOB_PROSE_BINARY` to the SHA-256-verified v0.6.15
release binary when running the Node suite to include the real policy/collect/
check API fixture. The matching source's `cargo test --locked -p safe-outputs
advisory` covers publisher stale-head rechecks and bounded publication with fake
forge responses. No live inference or write smoke test is part of local
verification.

### Ordinary dispatched reviews (dormant)

This is the wrapper slice of actions#174 / tracker#1171, not a caller
cutover. No pool, caller, admission or ruleset is activated here, and
`cgwalters-review.yml` remains unchanged. The concrete threats are spending
an unpaced fallback model, reading PR-owned policy with privilege, leaking
a GitHub token to hostile head code, and publishing a stale or unchecked
approval. Review dispatch therefore has a closed contract:

- Run `dispatch.yml` in the public **base repository**, on its default
  branch. Pass `kind: review`, `repo: OWNER/REPO` equal to that repository,
  positive `item`, exact lowercase 40-hex `head`, and nonempty `pool`.
  These are agent-board#620's ordinary sender inputs, including its fixed
  `task` hint; that hint is ignored, not review instructions. The caller
  names the run `review OWNER/REPO#ITEM HEAD (POOL)` for the pass reader.
- The base repository variable `REVIEW_POOLS` is the only pool map. Unset
  means every review dispatch fails. It is a JSON object, for example
  `{"openai":{"agent":"opencode","model":"provider/model","config":".github/agentic-job/review-hosted.toml"}}`.
  This is an example, not a deployed pool. Keys are lowercase letters,
  digits or hyphens, start with a letter, and are at most 32 characters.
  Each selected value has only `agent` (`opencode` or `claude`), nonempty
  single-word `model` (at most 200 characters, letters/digits and
  `_./:+-`), and optional `config`. Config is a relative TOML path without
  empty, `.` or `..` components; absent uses the pinned helper's
  `.agentic-job-source/.github/agentic-job/hosted.toml`. A base-owned custom
  config can name agent configuration repositories and setup. There is
  **no AGENT/AGENT_MODEL/AGENT_CONFIG fallback**, nor dispatched agent,
  model, runner, config, endpoint, budget or task override for reviews.
- The trusted dispatched default-branch SHA owns `workflow/review.toml`,
  `workflow/review-command.toml`, `workflow/review.md`, merge guidance and
  the selected config. Review always uses those fixed bounds/task paths.
  Admission accepts the shipped TOML subset: one-line JSON string arrays
  for `hosts`, `repos`, `bases` and trigger lists; a boolean `forks`;
  no duplicate admission keys or unknown trigger keys. Other policy/output
  settings are checked by the pinned policy helper. Unsupported admission
  syntax fails closed. Human authors still need OWNER/MEMBER/COLLABORATOR
  association and an admitted role (`all`, or their current permission).
  Bots always need both immutable `REVIEW_BOT_IDS` admission and their bot
  login in the bounds, even with a trusted association. Otherwise an exact `/review` comment must have a
  currently authorized admin/maintain/write author admitted by command
  bounds. Since #620 sends no command id, the wrapper checks at most 100
  comments (a full page is refused) and the latest ten candidate commands;
  the pass still owns command consumption, deduplication and pacing. The
  admitted automatic or command bounds file also governs policy/check;
  automatic bounds cannot override command-specific fork/output limits.
- PRs must be open, not draft, based on the base repository's default
  branch and still at `head`. Policy receives that expected head and checks
  it again. The hosted sandbox gets no GitHub token or provider key; the
  pinned runner fetches the exact head as data inside that sandbox, never
  as a privileged checkout. Reviews are capped at 150 inference requests,
  500 AIC and 75 minutes through policy; the agent job retains its existing
  outer timeout. Broker aggregate limits still apply.

Publication remains off unless `AGENT_REAL_REVIEWS=true`. Only a successful
agent and successful check with APPROVE or CHANGES reach `review submit`;
noop, cancellation and failed checks never publish. Submit downloads only
the policy binary and this run's checked artifact, uses the base repository
`GITHUB_TOKEN` (not the apply PAT), binds `commit_id`, and generates the
trusted run marker and agent-job link. A preflight also rechecks draft/base;
the helper rechecks open state/head immediately before submission. Review
never falls through to PAT apply or an unbound stale verdict comment.
The helper's existing 422 fallback may post a marked comment instead of a
review; that is not an approval and the pass must not accept it as one.

Before activation, callers need a separately reviewed full-SHA review call,
the matching broker **and** tailnet `job_workflow_ref` admissions retaining
owner-ID restrictions, base-owned bounds/task/config and explicit pools.
Keep the existing event caller and its admission for rollback. Local tests
exercise admission, publication wiring and preflight; they do not prove a
live tailnet join, inference registration, sandbox head fetch, Actions bot
identity/commit/run-marker acceptance by the pass, or GitHub submission.
Those require the coordinated `cgwalters-forge/review` pilot, including
stale-head, cancellation, failed-check/noop and authorized/unauthorized
command cases. Actions must be allowed to create/approve PRs. No ruleset,
legacy removal or rollout is part of this change.

### Draft classifier reviews: blocked on reusable helper wiring

The review-only draft extension requested in actions#200 is **not available**.
Ordinary dispatched and event reviews still reject drafts; no opt-in or
`agent-board-review-result-v1` writer job is exposed. The concrete threat is
laundering arbitrary agent prose (or an ordinary implementation run) into a
verified quick classification by attaching a trusted writer job name. An
ordinary checked APPROVE is not a checked draft-classifier result.

The binary prerequisite landed in agentic-job#3067 and v0.6.12
(`eecc8c434e67a7607264ab3f7d90c88b6d02042a`). The wrappers' current
`740ff7ff4dcef59dcb0619eb14d0e357c3193f62` pin carries it, and the
runner-only migration above is complete; neither an older pin nor the
removed sandbox user is the remaining blocker. The pinned binary's
[draft-classifier contract](https://github.com/cgwalters-forge/agentic-job/blob/740ff7ff4dcef59dcb0619eb14d0e357c3193f62/docs/review.md#draft-classifier-reviews)
requires `REVIEW_DRAFT_CLASSIFIER=true` in policy dispatch, check's verdict
step and submit, with `add_comment,noop` and `max-outputs: 1`.

The reusable workflows at that pin do not yet carry that setting or the
snapshot between stages. Specifically,
[`policy.yml`](https://github.com/cgwalters-forge/agentic-job/blob/740ff7ff4dcef59dcb0619eb14d0e357c3193f62/.github/workflows/policy.yml)
has no classifier input/environment on `Resolve a dispatched review` and
does not upload its `$RUNNER_TEMP/dispatch-review.json` with the policy
artifact.
[`check.yml`](https://github.com/cgwalters-forge/agentic-job/blob/740ff7ff4dcef59dcb0619eb14d0e357c3193f62/.github/workflows/check.yml)
has no classifier setting and invokes `review verdict` without
`--pull-request`. A wrapper environment cannot be passed through a reusable
workflow call. Setting a repository variable alone therefore enables nothing.

The precise prerequisite is upstream reusable-workflow plumbing for a trusted,
off-by-default classifier setting, a policy-owned snapshot uploaded by ID,
and check invoking the binary with that snapshot and setting. Then pin all
helper calls and the agent sequence together to a compatible commit, before
adding wrapper admission or the reserved writer name. Do not duplicate the
policy/check workflows here or substitute a snapshot from the agent. Keep
ordinary draft rejection, credential-free head fetching, no partial apply,
cancellation/failed-check/noop guards and the base `GITHUB_TOKEN` writer.
Submit must receive `BASE` and revalidate repository, PR, head, base and exact
title/description. This mode grants no fix, ready, promotion or merge authority.

The exact consumer contract is [agent-board#680's docs/poll.md](https://github.com/cgwalters-forge/agent-board/blob/71fe8272e70ba8438e46ba43336eed862bf7cdc6/docs/poll.md#draft-review-result-contract).
Only an actual approving review may produce `APPROVED` with `commit_id`, one
`agent-board-draft-review` JSON fence and one trusted gh-aw run marker. Check
must reject duplicate fences/JSON fields, unknown fields/versions, forged
markers, wrong targets and oversized results; enforce the 8192-byte **final
body** limit (including the writer's footer/marker) and nonempty single-line
evidence/reason of at most 1024 bytes each. The schema is
`agent-board/draft-review/v1`; its exact fields are `schema`, `repository`,
`pr`, `reviewed_head`, `title`, `description`, `semantic_diff`,
`description_assessment`, `change_category`, `risk`, `evidence` and `reason`,
plus optional `architecture_findings` (an array of nonempty single-line
strings, each at most 1024 bytes). A nonempty architecture finding forces deep.
Only `pass` / `matches` / `none` for semantics / description / risk can
support quick; conservative, unknown or negative findings cannot claim it.
Categories are `docs`, `dependencies`, `ci`, `mechanical` or
`narrow_fix_with_test`, not path-derived proof. The consumer still owns its
deterministic file/line/path/CI vetoes and reservation/run provenance checks.

After that workflow plumbing lands, update the wrapper's helper pins together,
retain literal privileged dependency pins, and reserve the exact writer job
name only for independently checked `kind=review` classifier output. Its
publication must occur within that successful job's latest-attempt interval;
a skipped, failed, cancelled or unchecked job cannot attest a result. Keep
head code out of the writer context. Add offline helper fixtures for forged,
duplicate, oversized and mismatched fields, title/description edits, closed/
retargeted/moved PRs, explicit draft admission and exact writer provenance
before exposing the opt-in. Local regressions currently cover rejection and
the ordinary writer boundaries, not a functioning classifier.

Caller migration is blocked too: do not enable draft intake or rename a
general job to satisfy the consumer. The four caller children and tracker#1324
remain separate and disabled until compatibility is proven. No caller
configuration, permissions, credentials or live reviews change here; the
wrapper does not yet expose the binary's classifier setting.

Runs may edit `.github/` here, so pull requests are reviewed by
[this repository's caller](.github/workflows/review.yml) of the wrapper
above. It uses a local reference so it runs the base branch's version, never
the pull request's version, with this repository's bounds and task in
[workflow/](workflow/review.toml). The ruleset on `main` is to require its
approval, which is dismissed when new commits are pushed. The approval is submitted
only with the repository variable `AGENT_REAL_REVIEWS=true`, alongside the
variables listed above. Dependabot's pull requests are reviewed only with
its ID in `REVIEW_BOT_IDS`. Without `.github/agentic-job/merge.toml` the
reviewer is told that a person will approve after its review.

## Lock new items

[lock-new-items](.github/workflows/lock-new-items.yml) locks each newly
opened issue, so that only collaborators can comment on
it and agent runs on it don't read outsiders' comments (tracker#733). Its one
step locks the item by API with the caller's job token: no PAT, and
none of the item is read or run. Pull requests are not locked: a locked
PR refuses reviews from the review job token, and the supervisor only
dispatches issues. It does not cover the opener's own
text, comments in the seconds before the lock, items opened by another
workflow's job token or transferred in (no `opened` event), or items that
predate the caller. A repository opts in with this one
file, `.github/workflows/lock.yml`, matching
[this repository's caller](.github/workflows/lock.yml) except for its `uses`:

```yaml
name: lock
on:
  issues:
    types: [opened]
permissions: {}
jobs:
  lock:
    uses: cgwalters-forge/actions/.github/workflows/lock-new-items.yml@main
    permissions:
      issues: write
```

The same pull request exempts the file in
`.github/agentic-job/workflows.toml`, creating it if needed, as this
repository does: without that, workflows-check's shape check refuses every
later pull request. Its zizmor needs the `ref-pin` policy for our own
workflows, as set in [.github/zizmor.yml](.github/zizmor.yml).

Licensed under either of [Apache-2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.
