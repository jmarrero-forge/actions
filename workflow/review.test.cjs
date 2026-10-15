const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const { runInNewContext } = require('node:vm');

const workflow = readFileSync('.github/workflows/jmarrero-review.yml', 'utf8');
const jobs = Object.fromEntries([...workflow.matchAll(/^  ([a-z]+):\n([\s\S]*?)(?=^  [a-z]+:\n|$(?![\s\S]))/gm)]
  .map(([, name, body]) => [name, body]));

function admits({ association = 'NONE', bot = false, id = 42, ids = '', draft = false, command } = {}) {
  const expression = jobs.policy.match(/    if: >-\n([\s\S]*?)\n    uses:/)[1];
  return runInNewContext(expression, {
    github: {
      event_name: command === undefined ? 'pull_request_target' : 'issue_comment',
      event: {
        pull_request: { draft, author_association: association, user: { type: bot ? 'Bot' : 'User', id } },
        issue: { pull_request: {} }, comment: { body: command || '' },
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

test('only admitted agent jobs compete, and only pushes cancel them', () => {
  assert.match(jobs.agent, /if: \$\{\{ needs.policy.outputs.admitted == 'true' \}\}/);
  assert.match(jobs.agent, /group: review-pull-\$\{\{ github.event.pull_request.number \|\| github.event.issue.number \}\}/);
  assert.match(jobs.agent, /cancel-in-progress: \$\{\{ github.event_name == 'pull_request_target' \}\}/);
});

test('trusted source only, one network join before dropping root, no post-agent steps', () => {
  assert.doesNotMatch(workflow, /pull_request.head|head.ref|secrets: inherit|workflow_dispatch/);
  const steps = [...jobs.agent.matchAll(/^      - (?:uses: ([^\s]+)|name: (.+)|id: (.+))/gm)]
    .map(([, uses, name, id]) => uses || name || id);
  assert.deepEqual(steps.slice(2), [
    './.agentic-job-source/prepare', "This workflow's own repository and commit", 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1',
    'Join the tailnet', './.agentic-job-source/secure-host', 'run',
  ]);
  assert.match(jobs.agent, /ref: \$\{\{ needs.policy.outputs.ref \}\}/);
  assert.doesNotMatch(jobs.agent, /(?:pull-requests|contents|issues): write|secrets\./);
  for (const name of ['policy', 'check', 'apply']) {
    assert.match(jobs[name], new RegExp(`uses: cgwalters-forge/agentic-job/\\.github/workflows/${name}\\.yml@[0-9a-f]{40} # v\\d+\\.\\d+\\.\\d+\\n`));
  }
  assert.match(jobs.agent, /\n {8}uses: \.\/\.jmarrero-actions\/tailscale\n/);
  assert.match(jobs.agent, /\n {10}repository: \$\{\{ job\.workflow_repository \}\}\n {10}ref: \$\{\{ job\.workflow_sha \}\}\n {10}path: \.jmarrero-actions\n/);
  for (const [name, body] of Object.entries(jobs)) {
    if (name !== 'apply') assert.doesNotMatch(body, /secrets\./);
  }
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

test('no fallback to another deployment: the variables or a failed policy', () => {
  assert.doesNotMatch(workflow, /100\.121\.|praxis|cgwalters-bot|uses: cgwalters-forge\/actions|AGENT_CONFIG_REPO|AGENT_CONFIG_PATH/);
  for (const [input, name] of [['agent', 'AGENT_REVIEW'], ['model', 'AGENT_MODEL'], ['inference-url', 'INFERENCE_URL'], ['inference-audience', 'INFERENCE_AUDIENCE']]) {
    assert.match(jobs.policy, new RegExp(`\\n {6}${input}: \\$\\{\\{ vars\\.${name} \\}\\}\\n`), input);
  }
  assert.match(jobs.agent, /\n {10}oauth-client-id: \$\{\{ vars\.TAILSCALE_OAUTH_CLIENT_ID \}\}\n {10}tags: \$\{\{ vars\.TAILSCALE_TAGS \}\}\n/);
});
