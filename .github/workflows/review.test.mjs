import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

// The review gate: a thin caller of this repository's cgwalters-review.yml,
// with this repository's copies of the bounds and the task, which that
// workflow reads from the base branch (agentic-job's docs/review.md,
// "Another repository").
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const caller = read('./review.yml');

// pull_request_target runs with write scopes: safe only while nothing here
// checks out or runs the head and no secret is handed down. So the caller
// is pinned whole, comments aside: one job, no steps, no secrets, and only
// the triggers and grants agentic-job's docs/review.md gives a caller. The
// `if:` admits every pull request event and `/review` comments; the called
// workflow narrows both (authors, and who may comment).
test('the caller runs nothing of the head and passes no secret', () => {
  const lines = caller.split('\n')
    .map(line => line.replace(/ # zizmor: .*$/, ''))
    .filter(line => line.trim() && !line.trim().startsWith('#'));
  assert.equal(lines.join('\n'), `name: review
on:
  pull_request_target:
    types: [opened, synchronize, reopened, ready_for_review]
  issue_comment:
    types: [created]
permissions: {}
jobs:
  review:
    if: >-
      github.event_name == 'pull_request_target' ||
      (github.event.issue.pull_request && startsWith(github.event.comment.body, '/review'))
    uses: ./.github/workflows/cgwalters-review.yml
    permissions:
      contents: read
      issues: read
      pull-requests: write
      actions: read
      id-token: write
      statuses: write`);
});

// One agentic-job release across our workflows, which dependabot moves
// together: its commit, since its tags can be moved (#109), and the tag as
// the comment dependabot updates with it.
test('the wrapper calls agentic-job at the commit the agent job calls', () => {
  const tags = file => [...read(file).matchAll(/uses: cgwalters-forge\/agentic-job\/\S+@(.*)/g)]
    .map(match => match[1]);
  const agent = tags('./cgwalters-agentic-job.yml');
  assert.equal(agent.length, 3);
  assert.match(agent[0], /^[0-9a-f]{40} # v\d+\.\d+\.\d+$/);
  assert.deepEqual(tags('./cgwalters-review.yml'), agent);
});

// Our own actions follow main (dependabot.yml), and zizmor accepts any ref
// for them, so a commit pin would never move: the review runs would keep
// joining with an old client, upstream pin and hardening.
test('both wrappers use our own actions at main', () => {
  for (const file of ['./cgwalters-agentic-job.yml', './cgwalters-review.yml']) {
    const refs = [...read(file).matchAll(/uses: cgwalters-forge\/actions\/(\S+)@(\S+)/g)];
    assert.deepEqual(refs.map(match => [match[1], match[2]]), [['admit', 'main'], ['tailscale', 'main']], file);
  }
});

// From v0.5.0 check.yml's detect job asks for `id-token: write`, and GitHub
// refuses a call that grants less than a called job asks for: every run
// would fail at startup. From v0.6.2 its check job asks for `actions: read`,
// to verify the proposals upload's digest. Nothing more than that, as no
// secret goes to check.
test('both wrappers grant check exactly what its jobs ask for', () => {
  for (const file of ['./cgwalters-agentic-job.yml', './cgwalters-review.yml']) {
    const check = read(file).split('\n  check:\n')[1].split(/\n {2}[a-z-]+:\n/)[0];
    assert.match(check, /\n {4}permissions:\n {6}contents: read\n {6}actions: read\n {6}id-token: write\n {4}(?:#[^\n]*\n {4})*uses: /, file);
    assert.doesNotMatch(check, /secrets/, file);
  }
});

// A called workflow's jobs get at most what the caller grants, so the caller
// grants what the wrapper's jobs ask for, and nothing more.
test('the caller grants exactly the union of the wrapper\'s job grants', () => {
  const grants = text => [...text.matchAll(/\n {4}permissions:\n((?: {6}\S.*\n?)+)/g)]
    .flatMap(match => match[1].trim().split('\n').map(line => line.trim().split(': ')));
  const union = {};
  for (const [scope, level] of grants(read('./cgwalters-review.yml'))) {
    if (union[scope] !== 'write') union[scope] = level;
  }
  assert.deepEqual(Object.fromEntries(grants(caller)), union);
});

// checkout reads an empty `repository` as the calling repository and an
// empty `ref` as its default branch, so a job that runs the policy call's
// source checks first that policy reported the repository it calls, at a
// full commit. Every such job, so that a recomposition cannot drop it.
test('every job checks the policy call\'s source before checking it out', () => {
  for (const file of ['./cgwalters-agentic-job.yml', './cgwalters-review.yml']) {
    const workflow = read(file);
    const called = workflow.match(/\n {4}uses: ([^/\s]+\/[^/\s]+)\/\.github\/workflows\/policy\.yml@/)?.[1];
    assert.ok(called, `${file}: policy call`);
    const jobs = workflow.split(/\n {2}(?=[a-z-]+:\n)/).slice(1);
    const checked = jobs.filter(job => job.includes('repository: ${{ needs.policy.outputs.source-repository }}'));
    assert.ok(checked.length >= 2, `${file}: jobs checking out the source`);
    for (const job of checked) {
      const name = job.split(':')[0];
      const steps = job.split(/\n {6}- /);
      const checkout = steps.findIndex(step => step.includes('repository: ${{ needs.policy.outputs.source-repository }}'));
      const guard = steps.slice(0, checkout).find(step => step.startsWith('name: The policy call\'s source repository'));
      assert.ok(guard, `${file}: ${name} checks the source first`);
      assert.match(guard, /\n {10}SOURCE_REPOSITORY: \$\{\{ needs\.policy\.outputs\.source-repository \}\}\n/, `${file}: ${name}`);
      assert.match(guard, /\n {10}SOURCE_SHA: \$\{\{ needs\.policy\.outputs\.source-sha \}\}\n/, `${file}: ${name}`);
      assert.equal(guard.match(/\n {10}EXPECTED_REPOSITORY: (\S+)\n/)?.[1], called, `${file}: ${name}`);
      assert.ok(guard.includes('[[ "$SOURCE_REPOSITORY" == "$EXPECTED_REPOSITORY" && "$SOURCE_SHA" =~ ^[0-9a-f]{40}$ ]] ||\n'
        + '            { echo "::error::the policy call is not $EXPECTED_REPOSITORY at a full commit"; exit 1; }'), `${file}: ${name}`);
    }
  }
});

test('the composed review links its pinned helper to the agent job', () => {
  const workflow = read('./cgwalters-review.yml');
  const agent = workflow.split('\n  agent:\n')[1].split('\n  check:\n')[0];
  const submit = workflow.split('\n  submit:\n')[1].split('\n  apply:\n')[0];
  assert.match(agent, /\n {6}job: \$\{\{ job\.check_run_id \}\}\n/);
  assert.match(submit, /\n {10}AGENT_JOB: \$\{\{ needs\.agent\.outputs\.job \}\}\n/);
});

// The bounds, comments aside: comment-only outputs for this repository's
// main, automatic events for any role (the shared workflow's `if:` narrows
// them to owner, members and collaborators), `/review` only from writers.
const bounds = name => read(`../../workflow/${name}.toml`)
  .split('\n').filter(line => line.trim() && !line.startsWith('#')).join('\n');
const common = repos => `hosts = ["github.com"]
repos = ${repos}
bases = ["main"]
max_outputs = 1
max_patch_bytes = 1024
max_patch_files = 1
[outputs]
add_comment = { max = 1 }
noop = { max = 1 }
[trigger]`;

test('the bounds admit one comment on this repository only', () => {
  const review = bounds('review');
  const command = bounds('review-command');
  const repos = review.match(/^repos = (\["[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+"\])$/m)?.[1];
  assert.ok(repos, 'repos names exactly one repository');
  assert.equal(review, `${common(repos)}
events = ["pull_request_target", "pull_request"]
roles = ["all"]
bots = ["dependabot[bot]"]
forks = true`);
  assert.equal(command, `${common(repos)}
events = ["issue_comment"]
roles = ["admin", "maintain", "write"]
bots = []
forks = true
commands = ["/review"]`);
});

// agentic-job's own task at the called tag, not agent-board's: that one says
// no person approves after the review, but whether one does here is the
// owner's merge policy (.github/agentic-job/merge.toml, `person` without
// it), which the run appends to the task.
test('the task treats the head as hostile and defers to the merge policy', () => {
  const task = read('../../workflow/review.md');
  for (const contract of ['hostile', 'Do not modify code', 'exact admitted head SHA', 'VERDICT: APPROVE', 'VERDICT: CHANGES', 'stated under "Merge policy"', 'Your output is only a comment']) {
    assert.ok(task.includes(contract), contract);
  }
  for (const claim of ['no person approves after you', 'the merge gate for every change here']) {
    assert.ok(!task.includes(claim), claim);
  }
});

// The review wrapper has no inputs and its caller passes no secret, so it
// stays on the broker: agentic-job's provider-key mode (its #740) is not
// wired here.
test('the review wrapper stays on the inference broker', () => {
  assert.doesNotMatch(read('./cgwalters-review.yml'), /INFERENCE_API_KEY|inference-register|inference-provider|inference-api-key/);
});
