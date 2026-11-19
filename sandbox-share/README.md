# sandbox-share

> This text is LLM generated.

Publishes a file, a directory tree or literal content as a root-owned,
world-readable copy under `/etc/agent-share/<name>`, so that the
sandboxed steps of a
[workflow-compiler](https://github.com/cgwalters-forge/workflow-compiler)
job can read it but not change it. It is what the compiler emits for a
`share` runner step; it runs as root through `sudo`,
before the job enters its sandbox.

```yaml
- uses: cgwalters-forge/actions/sandbox-share@main
  with:
    name: config
    path: ci/config.toml
```

Inputs:

- `name`: the name under `/etc/agent-share`, matching
  `[A-Za-z0-9][A-Za-z0-9._-]*`.
- `path`: a file or directory to copy, relative to the workspace.
- `content`: literal file content, used when `path` is empty.

The copy is built next to its final name and renamed into place, owned by
root with 0644 files and 0755 directories. A symlink or special file in
the source, or a `path` that goes through a symlink, is refused: a
checkout can hold a symlink to `/etc/shadow`, and following it as root
would publish the target to every user.

The Rust replacement and its result checker are in `action-scripts/`, with
an unprivileged test suite and a disposable-runner sudo dry run in CI. The
action remains on its current implementation until the first verified
release binary is available; see the
[delivery checkpoint](../tailscale/README.md#rust-migration-checkpoint).
The Rust implementation requires Linux 5.6 or later: it opens sources with
`openat2` without following symlinks, then copies from the opened descriptors.
A source path swapped after it is opened therefore cannot redirect the root
process to a private file; the copy reads only what was checked.
