# Reads between the runner's own processes

> This text is LLM generated.

Taken from agentic-job's `docs/secure-host.md`, for this action.

## Reads between the runner's own processes (opt-in)

The threat: every step runs as the runner's user, so without this a step
can read `/proc/PID/environ`, and the tokens in it, of any process an
earlier step left running. Yama decides only attaches, and `hidepid`
alone does not separate one user's processes.

`sandbox.deny-runner-reads = true` turns the rule on; unset, it follows
`lock-runner`, and this action's [`hosted.toml`](../hosted.toml) sets it. It
needs `sandbox.lock-runner`, because with sudo the runner's user could
clear the rule's map.

The rule is the world-write object's fifth program, `runner_read_check`,
a modify-return hook on `security_ptrace_access_check`. It refuses a
read (`PTRACE_MODE_READ` without `ATTACH`) by a process of the runner's
user of another of its processes, with two exceptions:

- A process may still read its own descendants, up to 32 generations
  down, the relationship Yama's scope 1 allows for attaching. An orphan
  has been reparented away from every later step.
- Runner.Worker is exempt, because it ends the job's orphans by reading
  each one's environment for `RUNNER_TRACKING_ID`.

The image's loader runs under conmon, not under the Worker, so it takes
the one non-root `Runner.Worker` on the host (`--sole-worker`) and
refuses if there is none, a root one or more than one. It also raises
Yama's `ptrace_scope` from 0 to 1, because attaches, `/proc/PID/mem`
among them, are Yama's to refuse. It proves the rule as the world-write
policy is proven: a process of the runner's user reads a sibling's
environment, which holds only `PATH`; the read must succeed before the
map is enabled and fail with `EACCES` after.

## What a step sees

The kernel decides setup's `hidepid=invisible` with the same check. So
under the rule a step does not see in `/proc` at all the runner's
processes that are not its descendants: `ps`, `pgrep` and `pkill` do not
find a server an earlier step started, and a process cannot read its
parent's `/proc` entries. A read of such a process fails with `ENOENT`
or `EACCES`. Signals still reach it.

## What it does not stop

- The Worker is exempt by its pid, which the pin keeps after the job. On
  a reused host, a later job's Worker is not exempt, and its orphan
  cleanup stops working.
- In the pinned image, a step can still signal an earlier process: it
  can raise the process's core limit and make it dump core where the
  host's `core_pattern` leaves the dump readable (below).
- It does not end the process: runner cleanup does, at the job's end.
- It is not isolation between steps: a step can still tamper with later
  steps, which the [threat model](threat-model.md) accepts.

## Forced core dumps

Even when the BPF rule and Yama refuse `/proc` reads and ptrace, a step
can use `prlimit` and `SIGSEGV` on another process of the same uid, then
read its memory through `coredumpctl dump` on a host using
systemd-coredump: a forced-core route to a step's parent, agentic-job's
`run` and the Worker, separate from the diagnostic-socket route below.

[#3154](https://github.com/cgwalters-forge/agentic-job/pull/3154),
at [773322d503987eac40845ce49d5bc0b9acbd8b30](https://github.com/cgwalters-forge/agentic-job/commit/773322d503987eac40845ce49d5bc0b9acbd8b30)
and in the pinned v0.6.14,
sets `kernel.core_pattern` to `|/bin/false` when `deny-runner-reads` is
enabled (`sandbox.deny-runner-reads` in this action's configuration,
`host.deny-runner-reads` by its newer name). It is not a new option:
the rule follows `lock-runner` unless set, is on in this action's
`hosted.toml`, and requires the runner lock. Only root in the initial
user namespace can change the host-wide pattern; the lock denies that
to later steps. The check fails `core-pattern` unless that exact
discard setting is readable, accepting a single trailing newline.

Every job on such a host loses **all new core dumps**, deterministic
CI included, not just dumps of the runner's processes. Existing stored
cores are not removed and remain readable according to their access
rules. Disabling `deny-runner-reads` gives up both the read rule and the
new discard behavior; it is not a per-process diagnostic exception.
The setting neither separates steps nor closes .NET diagnostic sockets.
All runner-mode credential refusals remain: inference-provider keys,
MCP credentials, the OIDC request and the other `--runner-home` refusals.

This repository's CI sees `core-pattern` pass on the pinned image
([evidence](evidence.md#adoption-checkpoint-2026-11-15)). Its `confined`
job forces a dump of a sacrificial child before and after the action;
for the Worker it relies on the host setting, never a crashed Worker.
It has passed on this image on main. RHEL is unverified.

## The Worker's diagnostic sockets

A related route into the Worker's memory. The .NET runtime binds
diagnostic sockets for `Runner.Worker` (`dotnet-diagnostic-PID-KEY-socket`
in its temporary directory) once, when the Worker starts. The Worker's
own user, which every step is, can connect to them and have the Worker
dump its memory, every secret of the job in it, or load a library into
it; Yama sees no ptrace attach in that.
`sandbox.unlink-worker-diagnostics` unlinks them, so the Worker keeps a
socket no step can name. It needs `sandbox.lock-runner` and, since the
pinned v0.6.14, follows it unless set, so it is on under this action's
`hosted.toml`, which sets `lock-runner`;
`sandbox check`'s `worker-diagnostics` probe tests it when it is on.

## Proof

This repository's `build` job
([`secure-host.yml`](../../.github/workflows/secure-host.yml)) has a step
on each side of the action: before it, one starts a detached `sleep`
with a canary in its environment and reads it; after it, the other must
fail to, with permission denied or no such file and nothing else, and
with the process alive by a signal check
before and after. The job must still end green with the process left
running. The `confined-cleanup` job checks, from the `confined` job's
log, that the runner's own cleanup ended the process that job left
([evidence](evidence.md)).
