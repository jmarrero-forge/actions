# renovate

> This text is LLM generated.

[forge.json](forge.json) is the default for every repository in
cgwalters-forge: forge-infra's daily self-hosted Renovate finds each
repository its app can see and onboards it with a `.github/renovate.json`
that extends `github>cgwalters-forge/actions//renovate/forge`. The preset is
bootc-dev's shared configuration (bootc-dev/infra's
`renovate-shared-config.json`: action pins, images, crates and the rest,
grouped into one pull request per repository) less its `:gitSignOff`, since
a bot adds no Signed-off-by. Midstream forks are left alone. Once a
repository is onboarded its Dependabot file can go.

[runners.json](runners.json) is a Renovate preset that moves GitHub-hosted
Ubuntu runner labels in `runs-on` to the newest one, and does nothing else.
Dependabot keeps the action pins and images in our repositories, so the
preset turns off every other Renovate update, and Renovate's own
vulnerability-alert pull requests as well, so that no pin gets two pull
requests. A repository adopts it with this one file,
`.github/renovate.json`, as [this repository](../.github/renovate.json)
does:

```json
{
  "$schema": "https://docs.renovatebot.com/renovate-schema.json",
  "extends": ["github>cgwalters-forge/actions//renovate/runners"]
}
```

The prerequisite is the Renovate GitHub App, installed for the repository
by an organization owner; this repository's runs cannot see whether it is.
Without it the file does nothing. Renovate groups a repository's stale
labels into one pull request, titled after the group, "Ubuntu runner
labels", when there is more than one update and after the one label
otherwise, and never merges it. The preset is read from this repository's
main branch, so a change to it here reaches every consumer at once.

It uses Renovate's `github-runners` datasource, which reads a label from a
literal `runs-on`, a string or a list of strings, in workflows (reusable
ones included) under `.github/workflows/` and in `workflow-templates/`.
`ubuntu-24.04` becomes `ubuntu-26.04` and `ubuntu-24.04-arm` becomes
`ubuntu-26.04-arm`: the architecture suffix is kept. Not touched:
`ubuntu-slim`, `ubuntu-latest` (which [lint](../lint/README.md) refuses),
macOS and Windows labels, a runner group, and labels that are not GitHub's.
Renovate does not read a label from an expression, so a matrix, a reusable
workflow's input or a variable (`AGENT_RUNNER` here) is not updated: change
those by hand. It does read each entry of a list, so a self-hosted runner
listed as `[self-hosted, ubuntu-24.04]` would be proposed for a move it
cannot make; review rejects that pull request, or the repository adds the
file to Renovate's `ignorePaths`.

## Checking it

`just test-renovate` runs Renovate's validator on the preset and on this
repository's configuration, then the opt-in Rust integration test
[renovate_dry_run.rs](../lint/tests/renovate_dry_run.rs): Renovate
offline over a copy of this checkout plus a fixture workflow, which must
propose the fixture's three stale Ubuntu labels and nothing else, none of
this repository's action pins, images or crates and none of the fixture's
other runners. npx downloads the pinned Renovate (which still needs Node),
and Cargo builds the test, so CI does not run this integration. The subprocess
has a five-minute timeout and uses a temporary checkout removed on success
or failure; it never writes the source checkout. Plain `just test-lint` runs
the offline log parsing, configuration and process-bound tests instead.

A consumer proves a real pull request with the app installed: put a job
with `runs-on: ubuntu-24.04` on the default branch of a sandbox repository
that has the file above, then run Renovate on it from the app's dashboard
(developer.mend.io), or wait for its next run. Expected: one open pull
request from `renovate[bot]`, its title naming `ubuntu`, that changes that
label to `ubuntu-26.04` and nothing else, with automerge off. Link it where the migration is tracked,
then close it.
