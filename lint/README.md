# lint

> This text is LLM generated.

The workflow check every forge repository runs the same way:
[actionlint](https://github.com/rhysd/actionlint) and
[zizmor](https://docs.zizmor.sh/) from images pinned by digest in
[Containerfile.pin](Containerfile.pin), then a runner-label
policy that denies `ubuntu-latest` in any `runs-on`. New workflows use
`ubuntu-26.04`; every other label, self-hosted and custom runners
included, is allowed.

```yaml
jobs:
  lint:
    runs-on: ubuntu-26.04
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: cgwalters-forge/actions/lint@main
```

It checks the repository checked out at the workspace root and needs
docker and cargo on the runner, as GitHub's Ubuntu runners have. zizmor
reads the repository's own `.github/zizmor.yml`, and runs its online
audits with the job's token (`contents: read` is enough): a hash pin to a
commit that exists only in a fork of the action's repository, or whose
`# vX` comment no longer matches it, fails the check.

## Runner labels

`ubuntu-latest` follows GitHub's choice of Ubuntu, so a workflow changes
under a repository without a commit. A job is refused when its `runs-on`
names a denied label: as a string, in a list or `labels`, as a quoted
literal inside an expression (`inputs.runner || 'ubuntu-latest'`), in the
job's matrix under any key when `runs-on` reads the matrix (`exclude`
aside), as the
default or a choice option of a `workflow_call` or `workflow_dispatch`
input that `runs-on` reads, or in the `with` of a job calling a reusable
workflow (followed through the matrix and inputs the same way). When any
of these reads `needs.JOB`, every string of that job's `outputs`, `env`
and steps' `run`, `with` and `env` counts too, one level deep, so a
dynamic matrix written to `$GITHUB_OUTPUT` is followed; a mention of a
denied label in that job's scripts for another purpose is refused as
well. Labels are compared case-insensitively, and `.YML` files are
checked as workflows-check takes them. It catches a label written by
mistake, not one assembled to get past it (`format()`, a repository
variable, a `needs` chain more than one job long).

The `deny` input adds labels to the list, separated by whitespace or
commas; `ubuntu-latest` stays on it whatever the input says.

## Locally

`just test-lint` also runs `tests/wrappers.rs`: its structural
contracts include tailnet broker defaults, broker-only review, artifact-only
research's no-publication wiring and checked-submit provenance. The research
outputs are checked by YAML key, so swapping two output names cannot pass.
These are static wiring checks, not live broker or publication tests. The
JavaScript execution tests (including research hand-back and submit preflight)
and Renovate's JavaScript-regex tests remain in the Node suite, alongside
review-policy assertions.

The runner-only session/provider-key declarations and credential wiring also
run in Rust: the legacy inputs remain declared for actionable refusals, only
the key's presence reaches target, and neither listener settings nor provider
credentials reach the agent. The inference URL/audience expressions have no
independent defaults. Node still executes the actual refusal and paired-default
cases; the Rust checks do not replace those behavioral tests.

`just test-lint` includes the Rust register-preflight harness in
`tests/register_preflight.rs`. It needs Python 3.11+ (`tomllib`) to execute
both wrappers' actual Python steps against temporary configuration files,
checking credential ordering, child-agent refusal, malformed/missing input,
the 1-MiB boundary and unchanged file contents. It acquires no credentials
and makes no network calls. The target job's JavaScript and its Node tests
remain separate until the production helper delivery is available.

The wrapper and Tailscale action's RHEL prerequisite tests share a Rust
harness in `tests/wrappers.rs`, run by `just test-lint` in CI. It executes
the actual Bash steps with fixture OS releases and a recording sudo double,
checking exact ordered arguments without installing packages or making
system links. This replaces `tailscale/prerequisites.test.mjs`; the action's
runtime JavaScript remains unchanged.

The same suite replaces `tailscale/hardening.test.mjs`, executing the action's
actual hardening shell with a Python sudo recorder (no root or network needed).
It checks always-run wiring after the join, exact ordered arguments for both
join outcomes, absent CLI/socket directories and failure of either chmod.
This tests shell control flow, not kernel permissions or a live tailnet join;
the runtime JavaScript tests remain in the Node suite.

Mint's action contract also runs in `tests/wrappers.rs`: the actual first
Bash step rejects conflicting login inputs and non-boolean mint values before
credentials or installation. YAML assertions check the default-off mint,
join/hardening/mint ordering, matching federated settings and key output.
No credentials or network are needed; `tailscale/mint.test.mjs` retains the
runtime HTTP exchange and key-validation tests until the helper port ships.

App-token's scope guard and permission forwarding run in
`tests/app_token_action.rs`. The Rust harness executes the actual Bash guard,
including Unicode whitespace refusals, and checks the parsed action's pin and
permission wiring. Node is still required to read remint's exported permission
list; its signing, cache and HTTP tests remain in `app-token/remint.test.mjs`.
No token is minted and no network call is made by the action-contract tests.

The Linux read-probe tests run in `tests/read_probe.rs`, invoking the actual
Node helper with a small API bridge. Rust checks the detached PID, canary,
tracking ID, session leadership, delayed readiness and fail-closed startup
(including timeout without a stuck IPC channel). Node and `/proc` are required;
no root or network is needed. This replaces `secure-host/read-probe.test.mjs`,
not the production helper or the hosted hardening acceptance test.

```sh
just --justfile path/to/actions/lint/Justfile --working-directory . check
```

`CONTAINER=podman` runs the linters with podman. Without `GH_TOKEN`
(`GH_TOKEN=$(gh auth token)`), zizmor runs offline and says so.

This repository's immutable wrapper self-pins use `# from main`: prose,
not an assertion that the reviewed commit is still the branch tip. Keep
that marker when updating the SHA; the scoped Renovate manager tracks
`main` separately. No audit exception is needed, and third-party version
comments and impostor-commit checks remain active.

With `GH_TOKEN` and zizmor 1.32.0 (the version in `Containerfile.pin`)
installed, run the opt-in online regression:
`just --justfile lint/Justfile test-online-self-pins`.
`ZIZMOR` may name its executable. It audits all seven wrapper self-pins,
then proves that reverting to `# main` fails against the advanced branch
and a wrong third-party version comment still fails. Ordinary
`just test-lint` stays offline; `just lint` runs the repository's online
audits when `GH_TOKEN` is set.

## workflows-check

The same crate builds the check an organization ruleset requires of every
pull request, so that a pull request cannot replace its own checker; see
[workflows-check.md](workflows-check.md).
