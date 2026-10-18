# actions

jmarrero-forge's fork of
[cgwalters-forge/actions](https://github.com/cgwalters-forge/actions):
GitHub Actions for agent runs, one per directory, plus the two reusable
workflows below, with jmarrero-forge's deployment settings in place of
cgwalters'. Callers pin them by commit.

| Action | Status | Roadmap |
| --- | --- | --- |
| [lint](lint/README.md) | actionlint, zizmor and the runner-label policy; this repository's CI calls it | Called from each forge repository's CI |
| [tailscale](tailscale/README.md) | Settings by organization; the jmarrero-forge and jmarrero entries take their client id and tags from the caller's inputs. CI runs its unit tests and checks the socket and CLI are closed after a join that fails, no real join (no tailnet credentials here) | Fill in jmarrero-forge's entry once its OIDC client exists |

## jmarrero-agentic-job

[jmarrero-agentic-job](.github/workflows/jmarrero-agentic-job.yml) is an
agent run of cgwalters-forge/agentic-job, pinned by the commit of one
release, with jmarrero-forge's tailnet join, inference proxy, runner and
agent. It is upstream's cgwalters-agentic-job with every fallback to
cgwalters' deployment removed: the agent is `claude` (or the scripted
`fake`, which needs none of the rest), and a real agent needs the calling
repository's variables below, or the matching inputs. A missing one fails
the run in its first job, naming it.

| Variable | |
| --- | --- |
| `AGENT` | `claude` (the default) or `fake` |
| `AGENT_MODEL` | optional, the model as the proxy serves it; empty for Claude Code's default |
| `INFERENCE_URL`, `INFERENCE_AUDIENCE` | the proxy, and the audience of the run's GitHub identity token there |
| `APPLY_ENVIRONMENT` | the environment gating apply, unless the caller passes `apply-environment` |
| `TAILSCALE_OAUTH_CLIENT_ID`, `TAILSCALE_TAGS` | the tailnet's OIDC client and node tags, for the join a real agent makes |
| `AGENT_RUNNER`, `AGENT_CONFIG` | optional, as upstream |

A repository gets agent runs with the workflow below, a bounds file at
`.github/agentic-job/dispatch.toml` (and `fix.toml` for the `fix` kind), a
runner configuration at `.github/agentic-job/hosted.toml`, and an
environment that gates the apply token; jmarrero-forge/tracker has all four.
The file's header says what else a caller may set.

The write token is the bot's PAT, stored only as the secret
`SAFE_OUTPUTS_PAT` of that environment (`agent-apply`), never as a
repository or organization secret. The caller still names it, as below,
but outside the environment the name resolves to nothing: only apply's
job, which runs in the environment, gets the PAT, to push to the bot's
fork and open the pull request. Everything else that would take
`SAFE_OUTPUTS_PAT` (policy's status comment, the session line, apply's
conclusion) gets an empty value and writes with the job token instead, so
it can comment only in the calling repository.

```yaml
name: dispatch
run-name: ${{ inputs.kind }} ${{ inputs.repo }}#${{ inputs.item }}
on:
  workflow_dispatch:
    inputs:
      task: {type: string, required: true}
      repo: {type: string, required: true}
      item: {type: string, required: true}
      kind: {type: choice, required: true, options: [implement, triage, research, review, fix]}
permissions: {}
concurrency:
  group: dispatch-${{ inputs.kind }}-${{ inputs.repo }}-${{ inputs.item }}
jobs:
  run:
    uses: jmarrero-forge/actions/.github/workflows/jmarrero-agentic-job.yml@COMMIT
    permissions: {contents: read, issues: write, pull-requests: write, actions: read, id-token: write}
    with:
      task: ${{ inputs.task }}
      repo: ${{ inputs.repo }}
      item: ${{ inputs.item }}
      kind: ${{ inputs.kind }}
      apply-environment: agent-apply
    secrets:
      SAFE_OUTPUTS_PAT: ${{ secrets.SAFE_OUTPUTS_PAT }}
```

## Review

[jmarrero-review](.github/workflows/jmarrero-review.yml) is the reusable
event-review wrapper. It composes agentic-job's policy, check, apply and
review helpers at the same commit as jmarrero-agentic-job, because
agentic-job's own review workflow joins cgwalters' tailnet, which has no
jmarrero-forge entry. It has **no inputs** and no fallbacks: the
variables `AGENT_REVIEW`, `AGENT_MODEL`, `INFERENCE_URL`, `INFERENCE_AUDIENCE`,
`TAILSCALE_OAUTH_CLIENT_ID` and `TAILSCALE_TAGS` configure it, as above,
and without `AGENT_REVIEW` the run fails in policy rather than have the scripted
agent post a verdict nobody made. `AGENT_RUNNER` (JSON) and `AGENT_CONFIG`
are optional.

Call it from the same `pull_request_target` and `issue_comment` events and
job condition as [this repository's caller](.github/workflows/review.yml),
with:

```yaml
uses: jmarrero-forge/actions/.github/workflows/jmarrero-review.yml@COMMIT
```

Keep that caller's six permission grants. `AGENT_REVIEW` is a variable of
its own so that turning on dispatched runs with `AGENT` does not also turn
on reviews, which the broker may not admit. The callers here pass no
secret: apply posts the review comment with the job token. Keep `workflow/review.toml`, `workflow/review-command.toml`
and `workflow/review.md` on the caller's base branch. Automatic reviews admit
non-draft owner/member/collaborator PRs. Otherwise-unassociated bots wait
unless `REVIEW_BOT_IDS` lists their numeric IDs **and** the bounds admit
their logins (including Dependabot). `/review` still requires write access
through event policy. Each review is bounded to 150 requests, 500 AIC and
75 agent minutes.

`AGENT_REAL_REVIEWS=true` opts into checked APPROVE/REQUEST_CHANGES
submission with the job token, at the admitted head; unset is off. Enable
GitHub Actions' permission to create/approve PRs for this feature. The
compatibility `review` status is not a merge gate or human approval. The
broker and the tailnet must admit this wrapper's `job_workflow_ref`.
`just test-review` checks the wrapper's admission and publication
boundaries; live tailnet joins and GitHub review writes are not exercised
locally.

This repository's own [review workflow](.github/workflows/review.yml)
calls the wrapper with its bounds and task in
[workflow/](workflow/review.toml), and stays off until the repository
variable `AGENT_REVIEW` is set. Without `.github/agentic-job/merge.toml` the
reviewer is told a person approves after it.

Licensed under either of [Apache-2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.
