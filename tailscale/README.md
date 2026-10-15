# tailscale

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

The job needs `permissions: id-token: write` for the OIDC login.

## Settings by organization

What a join needs is in [tailnets.json](tailnets.json), keyed by
`github.repository_owner`; none of it is secret. An organization with no
entry fails before joining, even when the inputs below are given: add an
entry for it. An entry may leave `oauth-client-id` and `tags` empty for
the caller to pass as inputs; a join with either still empty fails before
it starts. The audience is `api.tailscale.com/` followed by the
client ID. The node always joins with `--accept-dns=false`, which leaves
the runner's DNS alone.

## Inputs

A nonempty input overrides the organization's setting.

| Input | Meaning |
| --- | --- |
| `oauth-client-id` | Client ID of the OIDC federated identity (the only login passed on; no OAuth secret or auth key) |
| `audience` | Audience of the OIDC federated identity (default: from the client ID) |
| `tags` | Comma-separated tags for the node |
| `proxy` | Inference proxy URL |
| `hostname` | Node name (default: from the runner name) |
| `args` | Additional arguments to `tailscale up` |
| `check` | `true` to also check the login and the proxy, see below |

The `proxy` output is the inference proxy URL, for the caller to pass
on.

## Checking a tailnet's setup

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

tailscale/github-action installs into `/usr/local/bin`, which isn't in
sudo's `PATH` there, and needs `iptables`. Until that moves in here, run
this first:

```bash
sudo dnf install -y iptables
sudo ln -sfn /usr/local/bin/tailscale /usr/bin/tailscale
sudo ln -sfn /usr/local/bin/tailscaled /usr/bin/tailscaled
```
