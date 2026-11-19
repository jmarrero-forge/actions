# Evidence

> This text is LLM generated.

What has been seen to hold, where, and what has not. Taken from
agentic-job's `docs/secure-host.md` and `docs/sandbox-check.md`. Only
hosted Ubuntu 26.04 has run any of it; RHEL 10 is admitted by the image
but its runtime is unverified.

## This repository's CI

### Adoption checkpoint (2026-11-15)

[`Containerfile.pin`](../Containerfile.pin) on main at
`6e720e90708413982f90c1a396475946f5a31d0c`
([#271](https://github.com/cgwalters-forge/actions/pull/271)) selects
**v0.6.14**, digest
`sha256:02e5f14ff420da99004ddc3bea4dd6d4a521051389736416cd737e9f51525a42`.
`gh attestation verify` of that digest with the action's signer
(`--repo cgwalters-forge/agentic-job`, certificate identity
`https://github.com/cgwalters-forge/agentic-job/.github/workflows/release.yml@refs/heads/main`,
`--source-ref refs/heads/main`, `--deny-self-hosted-runners`) reports
source `489c69d53cc23b989131952c9e3ca4d5add4e2b5`, the
[v0.6.14 release](https://github.com/cgwalters-forge/agentic-job/releases/tag/v0.6.14)
commit, built by
[release run 43617086845, attempt 1](https://github.com/cgwalters-forge/agentic-job/actions/runs/43617086845/attempts/1).
That source contains host-table parsing
[#3144](https://github.com/cgwalters-forge/agentic-job/pull/3144)
(`420967bf3e26e64b5180e36899950b5df877fd8e`) and core protection
[#3154](https://github.com/cgwalters-forge/agentic-job/pull/3154)
(`773322d503987eac40845ce49d5bc0b9acbd8b30`).

In main's [run 43629704804](https://github.com/cgwalters-forge/actions/actions/runs/43629704804)
at that commit, the local `./secure-host` action verified and pulled the
digest above in every job that uses it:

| Passed Ubuntu 26.04 job | Evidence at that revision |
| --- | --- |
| [build / 132553381321](https://github.com/cgwalters-forge/actions/actions/runs/43629704804/job/132553381321) | setup's `core-dumps` stage writes `\|/bin/false`; 36 sandbox checks, `core-pattern` among them; `worker-diagnostics` setup stage unlinked the Worker's socket; another process's environment refused; GET allowed, POST refused by proxy, npm audit POST allowed; artifact upload; the runner's cleanup logs `Terminate orphan process` for the left process |
| [egress-policy / 132553381779](https://github.com/cgwalters-forge/actions/actions/runs/43629704804/job/132553381779) | 36 checks with `core-pattern`; replacement policy permits POST `/secure-host-ci` (origin 405), refuses other POST and npm audit; artifact upload |
| [publish / 132553493689](https://github.com/cgwalters-forge/actions/actions/runs/43629704804/job/132553493689) | build artifact `14126894154`, ZIP digest `4a64a9d6d14785b0abd559678c4774cb3adb0cfd95a7ba69812eb1ff8d8f4cd0` matched; commit-data validation passed |
| [world-write / 132553381534](https://github.com/cgwalters-forge/actions/actions/runs/43629704804/job/132553381534) | 34 checks with `core-pattern`; the root-owned mode-0666 file refuses the append |

The same run's wrapper jobs passed, but through agentic-job v0.6.13's
forwarder, which selected the **v0.6.9** digest
`sha256:008d34bf9598ef83d7859772200f26637814d2cd105028457208ddc87d69788b`:
they are not v0.6.14 acceptance. Moving that pin belongs to
[agentic-job#2070](https://github.com/cgwalters-forge/agentic-job/issues/2070)
and the release train's repin
([tracker#1673](https://github.com/cgwalters-forge/tracker/issues/1673)).

That run has no forced-dump control or JavaScript-step probe on this
image. The `confined` and `confined-cleanup` jobs added by
[#272](https://github.com/cgwalters-forge/actions/pull/272) passed in
main's push [run 43708182513](https://github.com/cgwalters-forge/actions/actions/runs/43708182513)
at `1577e41620061898bce95193189f59836c76beb6`, which adds the jobs and
keeps the pin; the action's `IMAGE` is the same digest:

| Passed Ubuntu 26.04 job | Evidence at that revision |
| --- | --- |
| [confined / 132793763908](https://github.com/cgwalters-forge/actions/actions/runs/43708182513/job/132793763908) | v0.6.14 control binary before the action: every route works, `core-dump: true` under systemd-coredump; setup's `core-dumps` stage writes `\|/bin/false` and `core-pattern` passes; the run step and agentic-job `489c69d` `confine-probe` main and post records show every route false, `core_pattern: "\|/bin/false"`, owner write still true |
| [confined-cleanup / 132794081491](https://github.com/cgwalters-forge/actions/actions/runs/43708182513/job/132794081491) | the confined job's log has `Terminate orphan process: pid (2571) (sleep)` for the process its first step left running |

`worker-core` there is still derived from the sacrificial dump result
and the host pattern, not a crashed Worker. RHEL 10 remains runtime-unverified.

### Workflow coverage

[The secure-host workflow](../../.github/workflows/secure-host.yml) runs
the pinned image on `ubuntu-26.04`, on every pull request and push to
main. Each job has a read-only token and no secrets and runs code as
root before the action on a fresh VM. Every claim has a positive control
before the action, a [Justfile](../../Justfile) recipe or a probe script
run with `before`, and the same check after it with `after`:

| Claim | Check | Job |
| --- | --- | --- |
| no sudo, no docker | `just secure-host-root` | `build` |
| no metadata service, while `github.com` answers | `just secure-host-metadata` | `build` |
| no read of an earlier step's process environment, which stays alive | two steps of [`secure-host.yml`](../../.github/workflows/secure-host.yml) | `build` |
| a POST refused by the proxy, a GET through | `just secure-host-egress` | `build` |
| the artifact handed over by ID with a ZIP-digest comparison and data validation | `publish` | `publish` |
| a caller's `config` is taken whole (one that keeps sudo) | `just secure-host-options` | `options` |
| an `egress-policy` file replaces the image's rules | `just secure-host-egress /secure-host-ci` | `egress-policy` |
| an invalid policy fails setup | `just secure-host-invalid-policy` | `invalid-egress-policy` |
| a root-owned mode-0666 file refuses an append after the action exits, while one of the runner's own takes it | `just secure-host-world-write` | `world-write` |
| a `run:` step and a JavaScript action's main and post steps get no root, metadata, earlier process's or parent's environment, memory or ptrace, forced core dump, or the Worker's environment, dump or diagnostic sockets | agentic-job's `secure-host probe control before`, then `confined` | `confined` |
| the runner's cleanup still ends a process an earlier step left running | its `Terminate orphan process` log line | `confined-cleanup` |

Each also passes `sandbox check` inside the action, but for `options`
and `invalid-egress-policy`, which turn it off.

The `publish` job in secure-host.yml compares upload-artifact's ZIP
digest, not the extracted file's SHA-256. It does not prove the
[README contract's](../README.md#the-contract) fixed
single-file consumer from agentic-job's
[Trusted artifact consumers](https://github.com/cgwalters-forge/agentic-job/blob/main/docs/workflows-check.md#trusted-artifact-consumers).
That contract passes one file named `artifact` by ID and file SHA-256
to a trusted data validator available without checkout or another action.
The [executable caller example](../../.github/workflows/example-plain-ci.yml)
uses standalone secure-host with `egress: write-policy` and exercises
that file-digest check and data validation with a commit ID, without
secrets or a release environment. That separate work is covered by
[#169](https://github.com/cgwalters-forge/actions/issues/169) and
[PR #171](https://github.com/cgwalters-forge/actions/pull/171).

## Run-time controls through the action on hosted Ubuntu 26.04

agentic-job's CI runs its image built from the commit under test, which
can be ahead of this action's pin, through this action with
`image-archive`:

- `world-write-runtime`: after the action has exited, an append to
  setup's probe file `/etc/agentic-job/world-write-probe/file` must fail
  with "Permission denied", and a mode-0666 file of the runner's own
  takes one.
- `confined`: a probe as the runner's user before the action (the
  control, in which sudo, `docker info`, writing a root-owned mode-0666
  file in `/var/tmp`, the metadata service and the Worker's diagnostic
  sockets all work), then after it from a `run:` step and from a
  JavaScript action's main and post steps. Each route must fail, a file
  of the runner's own must still take a write, and reading the
  environment of a process an earlier step left running must fail.
  `confined-cleanup` then reads that job's log for the runner's
  `Terminate orphan process` line for that process.
- `secure-host-write-policy`: a `run:` step and a JavaScript action read
  `https://example.com/` and are refused a POST to it by the proxy, and
  the job uploads an artifact, saves and restores a cache, and ends
  green.

A hosted runner serves no IPv6 metadata address, so that refusal has a
unit test of its rule and a probe without a control, not a hosted proof.

### Forced core dumps (upstream only)

The successful Ubuntu 26.04
[confined job 42182106140/127775120020](https://github.com/cgwalters-forge/agentic-job/actions/runs/42182106140/job/127775120020)
tested the image built from PR head
`4f7fd85a9377cdeb128e7e52f5ed25d70def9481`, merged by
[#3154](https://github.com/cgwalters-forge/agentic-job/pull/3154) as
[773322d503987eac40845ce49d5bc0b9acbd8b30](https://github.com/cgwalters-forge/agentic-job/commit/773322d503987eac40845ce49d5bc0b9acbd8b30).
Its control reads a random marker from a sacrificial child's forced dump
before setup. After setup, the run step and JavaScript main and post
steps cannot recover such a dump; their records show
`core_pattern: "|/bin/false"` and `core-dump: false`. The image's
`core-pattern` check also passes. The same job records successful
pre-setup parent environ/mem/ptrace and Worker environ controls, followed
by refusals in run/main/post.

`worker-core` is derived from the sacrificial dump result and the host
pattern, **not** a direct dump of Runner.Worker or a test of its own
`prlimit`. No Worker or parent is crashed. This is upstream evidence on
a source-built image, not proof for the pinned image or RHEL evidence.
The host-wide setting loses every new core dump, deterministic CI's
too, but does not erase previously stored dumps. The job remains the
trust unit; all runner-mode credential refusals stay in place.

The pinned v0.6.14 contains the fix, and its `core-pattern` check passes
in this repository's CI ([adoption checkpoint](#adoption-checkpoint-2026-11-15)),
where the `confined` job has also passed the forced-dump control and
refusals on the pinned digest.
See [runner reads](runner-reads.md#forced-core-dumps) for the mechanism
and limits.

## Where each probe has been seen to fail

agentic-job's `tests/sandbox_host.rs` removes one protection at a time,
as root, after setup and a clean check on hosted `ubuntu-26.04`, and
requires that exactly the probes guarding it fail. It grants the
sandbox user a sudoers rule; adds it to a group; moves the polkit rule
away; opens the runner's home; puts a job variable into the system
manager's environment; takes the user off the cron and at deny lists;
switches the world-write policy off in its map (which must fail
`world-write` and `runner-world-write`); resets ptrace; starts a stopped
unit; opens listeners. For the runner's user it adds a sudoers rule
after the deny rule, a polkit rule that grants it, and starts the daemon
of a group it is in (`docker`). For the network it loads no rules, the
rules of a host with no proxy, and rules naming another uid as the
proxy's.

The test runs as the runner's user, which after setup has no root to put
anything back with, so CI starts before setup a root shell behind a
Unix socket only that user's group may connect to. It stands for what
the lock cannot close: a root process the image already runs that does
as a local user says. Nothing in a job should start one.

The first time that test ran it found a probe that passed for the wrong
reason: `pkexec` started straight from `run0` refuses because its parent
is PID 1, whatever polkit says.

What the hosted runner cannot show:

- `environ-self` and `environ-runner` rest on the kernel's own check of
  who may read `/proc/PID/environ`; there is nothing to remove, and the
  controls are all the evidence.
- It has no tailscaled and is on no tailnet; those rows passed by hand
  on a RHEL 10 runner (2026-10-07), under SELinux.
- Neither user gets an HTTP answer from the WireServer, so
  `egress-wireserver` and `egress-proxy-wireserver` have never had a
  control or been seen to fail. Its DNS port answers every user, which
  gives `egress-wireserver-dns` a control.
- It runs AppArmor, not SELinux.

## Timing

One run's step durations on hosted Ubuntu 26.04, not bounds. In
agentic-job's merge-queue
[run 38997639999](https://github.com/cgwalters-forge/agentic-job/actions/runs/38997639999/job/117294141312),
secure-host took 69 seconds, setup 48.4 and the probes 19.8; before the
world-write walk went it took 218. Loading and proving the world-write
policy takes under a second.

## RHEL 10

RHEL 10 is admitted by the image but runtime-unverified: there is no
RHEL runner for CI. Acceptance needs an operator-provisioned fresh RHEL
10 VM with root, kernel BTF and BPF privileges, never a persistent host
or a container. From a reviewed agentic-job checkout:

```sh
just world-write-test
export GITHUB_STEP_SUMMARY="$(mktemp)"
just world-write-evidence "$(mktemp -d)"
```

The summary must show every baseline, active-link, denied-write and
owner, sticky, tmpfs and overlay control passing, with kernel, package
versions, object provenance and timings. Destroy the VM afterwards, even
after a failure. The proxy's `:Z` mounts are also untested there.
