const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const { runInNewContext } = require('node:vm');

// The agentic-job release both wrappers call, by commit with its tag as a
// comment (a tag can be moved, #109): a bump changes it here, in the six
// calls, the review wrapper's header and the README.
const PIN = '258dc11ca7fe1798836976c0424274d1498e22cb # v0.6.2';

const workflow = readFileSync('.github/workflows/cgwalters-review.yml', 'utf8');
const jobs = Object.fromEntries([...workflow.matchAll(/^  ([a-z]+):\n([\s\S]*?)(?=^  [a-z]+:\n|$(?![\s\S]))/gm)]
  .map(([, name, body]) => [name, body]));

function admits({ association = 'NONE', bot = false, id = 42, ids = '', draft = false, command, visibility = 'public' } = {}) {
  const expression = jobs.policy.match(/    if: >-\n([\s\S]*?)\n    uses:/)[1];
  return runInNewContext(expression, {
    github: {
      event_name: command === undefined ? 'pull_request_target' : 'issue_comment',
      event: {
        pull_request: { draft, author_association: association, user: { type: bot ? 'Bot' : 'User', id } },
        issue: { pull_request: {} }, comment: { body: command || '' },
        repository: visibility === null ? {} : { visibility, private: visibility !== 'public' },
      },
    },
    vars: { REVIEW_BOT_IDS: ids }, fromJSON: JSON.parse,
    // GitHub contains casts values to strings and compares ignoring case.
    // This is an admission regression, not a full Actions expression parser.
    contains: (array, value) => array.some(item => String(item).toLowerCase() === String(value).toLowerCase()),
    startsWith: (value, prefix) => value.toLowerCase().startsWith(prefix.toLowerCase()),
  });
}

test('automatic reviews admit trusted associations, not arbitrary contributors or drafts', () => {
  for (const association of ['OWNER', 'MEMBER', 'COLLABORATOR', 'CONTRIBUTOR', 'FIRST_TIMER', 'NONE']) {
    assert.equal(admits({ association }), ['OWNER', 'MEMBER', 'COLLABORATOR'].includes(association));
    assert.equal(admits({ association, draft: true }), false);
  }
});

test('unassociated bots wait unless their id is configured; trusted associations retain upstream admission', () => {
  for (const [bot, ids, expected] of [
    [true, '', false], [true, '[42]', true], [false, '[42]', false],
    [true, '[142]', false], [true, '["42"]', true], [true, '42', false],
  ]) assert.equal(admits({ bot, ids }), expected);
  assert.equal(admits({ bot: true, ids: '[42]', draft: true }), false);
  assert.equal(admits({ bot: true, association: 'COLLABORATOR' }), true);
});

test('commands reach event policy, which retains the caller bounds and write-role check', () => {
  assert.equal(admits({ command: '/review' }), true);
  assert.equal(admits({ command: 'please /review' }), false);
  assert.match(jobs.policy, /event: true/);
  assert.match(jobs.policy, /'workflow\/review-command.toml' \|\| 'workflow\/review.toml'/);
  assert.match(jobs.policy, /max-requests: '150'[\s\S]*budget: '500'[\s\S]*timeout: '75'/);
});

// agentic-job's opt-in for private repositories (its #739) stays off here,
// whatever a caller's bounds say: nothing runs unless the repository is public.
test('a private or internal repository is never reviewed', () => {
  for (const visibility of ['private', 'internal', null]) {
    assert.equal(admits({ association: 'OWNER', visibility }), false, String(visibility));
    assert.equal(admits({ command: '/review', visibility }), false, String(visibility));
  }
  assert.equal(admits({ association: 'OWNER' }), true);
});

test('only admitted agent jobs compete, and only pushes cancel them', () => {
  assert.match(jobs.agent, /if: \$\{\{ needs.policy.outputs.admitted == 'true' \}\}/);
  assert.match(jobs.agent, /group: review-pull-\$\{\{ github.event.pull_request.number \|\| github.event.issue.number \}\}/);
  assert.match(jobs.agent, /cancel-in-progress: \$\{\{ github.event_name == 'pull_request_target' \}\}/);
});

test('trusted source only, one network join before dropping root, no post-agent steps', () => {
  assert.doesNotMatch(workflow, /pull_request.head|head.ref|secrets: inherit|workflow_dispatch/);
  const steps = [...jobs.agent.matchAll(/^      - (?:uses: ([^\s]+)|name: (.+)|id: (.+))/gm)]
    .map(([, uses, name, id]) => uses || name || id);
  assert.deepEqual(steps, [
    'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
    "The policy call's source repository",
    'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
    './.agentic-job-source/prepare', 'Join the tailnet', 'Check inference proxy reachability',
    './.agentic-job-source/secure-host/internal', 'run',
  ]);
  assert.match(jobs.agent, /ref: \$\{\{ needs.policy.outputs.ref \}\}/);
  assert.doesNotMatch(jobs.agent, /(?:pull-requests|contents|issues): write|secrets\./);
  for (const name of ['policy', 'check', 'apply']) {
    assert.match(jobs[name], new RegExp(`uses: [^\\n]+/${name}\\.yml@${PIN.replaceAll('.', '\\.')}`));
  }
  for (const [name, body] of Object.entries(jobs)) {
    if (name !== 'apply') assert.doesNotMatch(body, /secrets\./);
  }
});

test('both composed jobs retain the release pins and check the proxy between join and setup', () => {
  assert.match(PIN, /^[0-9a-f]{40} # v\d+\.\d+\.\d+$/);
  for (const file of ['cgwalters-agentic-job.yml', 'cgwalters-review.yml']) {
    const source = readFileSync(`.github/workflows/${file}`, 'utf8');
    const calls = [...source.matchAll(/uses: (\S+\/(?:policy|check|apply)\.yml)@(.*)/g)];
    assert.equal(calls.length, 3, file);
    for (const [, , ref] of calls) assert.equal(ref, PIN, file);
    const agent = source.split('\n  agent:\n')[1].split(/\n  [a-z]+:\n/)[0];
    const check = 'run: agentic-job secure-host inference .agentic-job/config.toml';
    assert.equal(agent.split(check).length - 1, 1, file);
    assert.ok(agent.indexOf('name: Join the tailnet') < agent.indexOf(check), file);
    const secureHost = 'uses: ./.agentic-job-source/secure-host/internal';
    assert.ok(agent.indexOf(check) < agent.indexOf(secureHost), file);
    assert.ok(agent.indexOf(secureHost) < agent.indexOf('uses: ./.agentic-job-source/run'), file);
    assert.doesNotMatch(agent, /steps\.secure-host\.outputs\.session-endpoint/);
  }
});

// Since v0.6.2 the public secure-host action forwards to the image action
// and fails on `binary`; internal keeps the second user `run` expects. Its
// check needs the proposals upload's digest (its grant: review.test.mjs).
test('both agent jobs use secure-host/internal and hand the proposals digest to check', () => {
  for (const file of ['cgwalters-agentic-job.yml', 'cgwalters-review.yml']) {
    const source = readFileSync(`.github/workflows/${file}`, 'utf8');
    const job = name => source.split(`\n  ${name}:\n`)[1].split(/\n  [a-z]+:\n/)[0];
    const agent = job('agent');
    assert.deepEqual([...agent.matchAll(/uses: (\.\/\.agentic-job-source\/secure-host\S*)/g)].map(m => m[1]),
      ['./.agentic-job-source/secure-host/internal'], file);
    assert.match(agent, /\n {6}safe-outputs-artifact-digest: \$\{\{ steps\.run\.outputs\.safe-outputs-artifact-digest \}\}\n/, file);
    assert.match(job('check'), /\n {6}safe-outputs-artifact-digest: \$\{\{ needs\.agent\.outputs\.safe-outputs-artifact-digest \}\}/, file);
  }
});

test('steering receives the dispatch item through policy metadata, or the admitted review event', () => {
  const dispatch = readFileSync('.github/workflows/cgwalters-agentic-job.yml', 'utf8');
  const policy = dispatch.split('\n  policy:\n')[1].split('\n  agent:\n')[0];
  assert.match(policy, /^      issue: \$\{\{ inputs.item \}\}$/m);
  assert.match(policy, /^      output-repo: \$\{\{ inputs.repo \}\}$/m);
  assert.match(jobs.policy, /^      event: true$/m);
  // Opt-in remains the trusted configuration's, not a new dispatcher input.
  for (const source of [dispatch, workflow]) assert.doesNotMatch(source, /watch-item:/);
});

test('real reviews are opt-in and use only the checked artifact at the admitted head', () => {
  assert.match(jobs.submit, /vars.AGENT_REAL_REVIEWS == 'true'/);
  assert.match(jobs.submit, /needs.check.result == 'success' && needs.check.outputs.artifact-id != ''/);
  assert.match(jobs.submit, /artifact-ids: \$\{\{ needs.check.outputs.artifact-id \}\}/);
  assert.match(jobs.submit, /HEAD_SHA: \$\{\{ needs.policy.outputs.review-head \}\}/);
  assert.match(jobs.submit, /return await submitReview\(process.env, output, github, core\)/);
  assert.match(jobs.apply, /!cancelled\(\)[^\n]+needs.submit.outputs.review-url == ''/);
  assert.match(jobs.status, /needs.agent.result != 'cancelled'/);
  assert.match(jobs.status, /await reportStatus\(process.env, github, core\)/);
});
