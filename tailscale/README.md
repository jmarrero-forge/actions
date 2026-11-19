# tailscale

> This text is LLM generated.

Joins the tailnet with
[tailscale/github-action](https://github.com/tailscale/github-action)
v4, with the settings of the workflow's organization, then makes
`/var/run/tailscale` root's only (0700).

tailscaled's LocalAPI socket is 0666, and the API doesn't check who
dials through it: any local user could `tailscale nc` to every node the
runner's tags may reach, as this node, and list the tailnet with
`tailscale status`. Only root needs it; use `sudo tailscale ...`.
tailscaled leaves an existing directory's mode alone when it restarts.
The socket is open from tailscaled's start until this action closes it,
and a connection made meanwhile survives that, so join before any other
user's processes start.

```yaml
- uses: cgwalters-forge/actions/tailscale@main
  id: tailnet
  with:
    hostname: cgwalters-devspace-${{ github.run_id }}
- run: your-command
  env:
    INFERENCE_PROXY: ${{ steps.tailnet.outputs.proxy }}
```

The job needs `permissions: id-token: write` for the OIDC login, or a
key minted by an earlier job that had it ([below](#a-job-without-an-identity-token)).

## Settings by organization

What a join needs is in [tailnets.json](tailnets.json), keyed by
`github.repository_owner`; none of it is secret. Another deployment's
organization, which has no entry, must give both `oauth-client-id` and
`tags` as inputs, or it fails before joining; its `proxy` is empty unless
given. The reusable workflows pass the calling repository's variables
`AGENTIC_JOB_TAILSCALE_OAUTH_CLIENT_ID` and `AGENTIC_JOB_TAILSCALE_TAGS`
as those inputs, but run only for an owner that
[admit/owners.json](../admit/owners.json) lists. The audience is `api.tailscale.com/` followed by the
client ID. The node always joins with `--accept-dns=false`, which leaves
the runner's DNS alone.

## Inputs

A nonempty input overrides the organization's setting.

| Input | Meaning |
| --- | --- |
| `oauth-client-id` | Client ID of the OIDC federated identity (no OAuth secret is ever passed on) |
| `authkey` | A key `mint` made in an earlier job, joined with in place of the federated login |
| `mint` | `true` to mint such a key after the join, as the `key` output |
| `audience` | Audience of the OIDC federated identity (default: from the client ID) |
| `tags` | Comma-separated tags for the node |
| `proxy` | Inference proxy URL |
| `hostname` | Node name (default: from the runner name) |
| `args` | Additional arguments to `tailscale up` |
| `check` | `true` to also check the login and the proxy, see below |

The `proxy` output is the inference proxy URL, for the caller to pass
on.

## A job without an identity token

A job whose later steps run code it does not trust, such as an agent
that is the runner's user, must not have `id-token: write`: anything
running as that user could mint identity tokens for any audience. Such a
job joins with a key instead. An earlier job, with the identity token,
joins with `mint: 'true'`; after its join the action exchanges a fresh
identity token and asks Tailscale for a key like the one the CLI makes
from a federated login: single-use, ephemeral and preauthorized, with the
same tags, valid for an hour. The `key` output goes to the later job only
as a secret of a reusable workflow call, which masks it in that job's
log, and that job passes it as `authkey`:

```yaml
jobs:
  register:
    permissions:
      id-token: write
    outputs:
      key: ${{ steps.tailnet.outputs.key }}
    steps:
      - uses: cgwalters-forge/actions/tailscale@main
        id: tailnet
        with:
          mint: 'true'
  agent:
    needs: register
    uses: ./.github/workflows/agent.yml
    secrets:
      TAILNET_KEY: ${{ needs.register.outputs.key }}
# and in agent.yml's job, before anything untrusted runs:
#     - uses: cgwalters-forge/actions/tailscale@main
#       with:
#         authkey: ${{ secrets.TAILNET_KEY }}
```

The join spends the key, so it is worth nothing to what runs after it.
The output is not masked, since GitHub drops a job output holding a
masked value, and nothing prints it; with step debug logging on the
runner would log it as it is set, so `mint` then refuses. `mint` with
`authkey` is refused too.

Keys are valid for one hour **from minting**, not from the later job's start.
A queue or runner-capacity wait uses that lifetime. Re-running just the later
job can reuse the earlier job's output: it does not mint a new key, and a
successful earlier join has already spent it. For the agent wrappers,
[re-run all jobs](../README.md#retry-recovery) after clearing the queue so the
trusted register job issues a new key. Do not give the untrusted job OIDC or
standing mint credentials to work around this limitation.

## Checking a tailnet's setup

`just test-lint` also runs `lint/tests/tailscale_check.rs`, which replaces the
JavaScript check test suite. Its bounded Node bridge exercises the shipped
`check.mjs` offline: audience replacement, scope-only logging, an unauthenticated
proxy POST with redirects refused, and early failure on bad responses. Run it
alone with `cargo test --locked --manifest-path lint/Cargo.toml --test tailscale_check`.
It checks request signals, not elapsed timeout behavior or live connectivity.

`just test-lint` runs the mint protocol tests in Rust, invoking the shipped
`mint.mjs` through a small Node bridge with synthetic Fetch responses. They
cover the OIDC exchange, single-use ephemeral key request, debug refusal,
malformed credentials and failures at each HTTP stage without real credentials
or network access. Node remains required until the production helper is ported;
these tests do not prove a live federation or tailnet join. For only this suite,
run `cargo test --locked --manifest-path lint/Cargo.toml --test tailscale_mint`.

With `check: 'true'` the action, after joining, exchanges a fresh OIDC
token to print the scopes Tailscale grants it, and sends the proxy a
`POST /v1/runs` without a token, which has to answer HTTP 401. No token
or response body is printed. A `workflow_dispatch` workflow with that
one step is enough to try a new organization's federated credential.

## Leaving the tailnet

The action doesn't log out, and is for disposable runners only.
tailscale/github-action's cleanup runs `sudo tailscale logout` at the
end of the job, which fails once a later step has taken sudo from the
job user. That cleanup is skipped when it can't run `tailscale
--version`, so this action makes `/usr/local/bin/tailscale` root's only
(0700) as well: the cleanup then logs that it skips, with no warning.

Nothing is lost on a runner that is thrown away: at the pinned commit an
OIDC join is `preauthorized=true&ephemeral=true` and tailscaled keeps
its state in memory (`--state=mem:`), so the node stays connected until
the runner is destroyed and Tailscale then removes it. On a persistent
runner tailscaled would keep running. Don't make the CLI accessible to
the job user again later in the job, or put another `tailscale` in its
`PATH`: the cleanup would run again.

## RHEL runners

On RHEL 10 (`ID=rhel` and `VERSION_ID` 10 or 10.x in `/etc/os-release`)
the action first installs `iptables`, which tailscaled needs, and links
`/usr/bin/tailscale{,d}` to the binaries tailscale/github-action
installs into `/usr/local/bin`, which isn't in sudo's `PATH` there.
The link leads to the CLI this action makes root's only, so the cleanup
still skips. Other systems get nothing extra.

## Rust migration checkpoint

`action-scripts/` contains the Rust replacements for settings and the optional
identity/proxy check. The action still uses the existing scripts until the
first release binary has been published and its SHA-256 pinned in this
repository. The Rust setup reads this action's `tailnets.json` at runtime;
deployment addresses are not compiled into the executable. Its network check
has a 30-second timeout per request and a 1 MiB limit on identity JSON bodies.

Delivery will use a release binary verified against a checked-in SHA-256,
not a checksum fetched alongside the download. This prevents replacement of
a release asset from changing code run by jobs. A host binary avoids a
container runtime and host bind mounts for the privileged share operation,
and does not require a Rust toolchain on callers. Publication, Linux
architecture support, download verification and action cutover remain to be
completed; no caller inputs or outputs change.
