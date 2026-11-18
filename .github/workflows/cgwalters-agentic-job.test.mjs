import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

// Non-JavaScript wrapper tests live in lint/tests/{wrappers,register_preflight}.rs.
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const workflow = readFileSync(new URL('./cgwalters-agentic-job.yml', import.meta.url), 'utf8');

// Renovate's matchStrings are JavaScript regular expressions, so this one
// runs them here rather than in lint/tests/wrappers.rs.
test('Renovate tracks the wrappers\' own-action pins from main without automerge', () => {
  const config = JSON.parse(read('../renovate.json'));
  assert.equal(config.customManagers.length, 1);
  const manager = config.customManagers[0];
  assert.equal(manager.customType, 'regex');
  assert.deepEqual(manager.managerFilePatterns, ['/^\\.github/workflows/cgwalters-(?:review|agentic-job|agent)\\.yml$/']);
  const eligible = new RegExp(manager.managerFilePatterns[0].slice(1, -1));
  for (const file of ['cgwalters-review', 'cgwalters-agentic-job', 'cgwalters-agent']) {
    assert.ok(eligible.test(`.github/workflows/${file}.yml`));
  }
  assert.ok(!eligible.test('.github/workflows/review.yml'));
  assert.equal(manager.datasourceTemplate, 'git-refs');
  assert.equal(manager.packageNameTemplate, 'https://github.com/cgwalters-forge/actions');
  assert.equal(manager.currentValueTemplate, 'main');
  const pattern = new RegExp(manager.matchStrings[0], 'g');
  const wrapper = read('./cgwalters-review.yml');
  const pins = [...wrapper.matchAll(pattern)];
  assert.equal(pins.length, 4);
  for (const pin of pins) assert.match(pin.groups.currentDigest, /^[0-9a-f]{40}$/);
  // Exercise successive digest-only updates, including after main advances
  // again: preserve paths and the prose marker, never generate `# main`.
  for (const file of ['./cgwalters-review.yml', './cgwalters-agentic-job.yml', './cgwalters-agent.yml']) {
    const original = read(file);
    const count = [...original.matchAll(pattern)].length;
    let updated = original;
    for (const digest of ['a'.repeat(40), 'b'.repeat(40)]) {
      updated = updated.replace(pattern, match => match.replace(/[0-9a-f]{40}/, digest));
      assert.equal([...updated.matchAll(pattern)].filter(pin => pin.groups.currentDigest === digest).length, count);
      assert.equal(updated.replace(pattern, '<self-pin>'), original.replace(pattern, '<self-pin>'));
      assert.doesNotMatch(updated, /@[0-9a-f]{40} # main(?:\r?\n|$)/);
    }
  }
  // admit and deployment for the dispatch wrapper's review path, and each
  // wrapper's register job's tailnet join; the shared agent job's join.
  for (const [file, count] of [['./cgwalters-agentic-job.yml', 3], ['./cgwalters-agent.yml', 1]]) {
    assert.equal([...read(file).matchAll(pattern)].length, count, file);
  }
  assert.deepEqual(config.packageRules, [{
    description: "Update the review wrapper's own-action commits together through review, even without release tags",
    matchManagers: ['custom.regex'], matchDatasources: ['git-refs'],
    matchPackageNames: [manager.packageNameTemplate], enabled: true,
    groupName: 'review actions', automerge: false,
  }]);
});

test('opencode or claude with no configuration repository fails in the target job', async () => {
  for (const agent of ['opencode', 'claude']) {
    await assert.rejects(target({ AGENT_CONFIG_REPO: '', AGENT: agent }), /No agent configuration repository/, agent);
    const outputs = await target({ AGENT: agent });
    assert.notEqual(outputs['agent-config-repo'], '', agent);
  }
  const outputs = await target({ AGENT_CONFIG_REPO: '', AGENT: 'vibe', AGENT_MODEL: 'devstral-small-latest' });
  assert.equal(outputs['agent-config-repo'], '');
});

// The target job's actual script, with doubles for github-script's globals.
const script = workflow.split('          script: |\n')[1].split(/\n\n|\n {6}- /)[0]
  .split('\n').map(line => line.replace(/^ {12}/, '')).join('\n');
const AsyncFunction = (async () => {}).constructor;

const head = 'a'.repeat(40);
const basePolicy = `hosts = ["github.com"]
repos = ["caller/repo"]
bases = ["main"]
[github.trigger]
events = ["pull_request_target"]
roles = ["all"]
bots = ["dependabot[bot]"]
forks = true`;

async function target(env, { pull = false, caller = { private: false, visibility: 'public', default_branch: 'main' },
  pr = {}, policy = basePolicy, commandPolicy, comments = [], permission = 'write', eventName = 'workflow_dispatch', ref = 'refs/heads/main',
  allowPolicyRead = true, item = {},
} = {}) {
  const outputs = {};
  const core = { setOutput: (name, value) => { outputs[name] = value; } };
  const github = { rest: {
    issues: { get: async ({ issue_number }) => ({
      data: { number: issue_number, title: 't', body: 'b', user: { login: 'item-author' }, author_association: 'MEMBER', ...(pull ? { pull_request: {} } : {}), ...item },
    }), listComments: async () => ({ data: comments }) },
    pulls: { get: async () => ({ data: {
      number: 7, state: 'open', draft: false, head: { sha: head, repo: { full_name: 'fork/repo' } },
      base: { ref: 'main', repo: { full_name: 'caller/repo' } }, author_association: 'MEMBER',
      user: { type: 'User', login: 'author', id: 12 }, ...pr,
    } }) },
    repos: { get: async ({ owner, repo }) => {
      assert.equal(`${owner}/${repo}`, 'caller/repo');
      return { data: caller };
    }, getContent: async ({ path, ref: policyRef }) => {
      assert.ok(allowPolicyRead, 'rejected target must not reach policy reads');
      assert.equal(policyRef, 'b'.repeat(40), 'read policy at dispatched trusted SHA, never head');
      assert.ok(['workflow/review.toml', 'workflow/review-command.toml'].includes(path));
      const text = path.endsWith('review-command.toml')
        ? commandPolicy ?? policy.replace('["pull_request_target"]', '["issue_comment"]').replace('["all"]', '["write", "maintain", "admin"]').concat('\ncommands = ["/review"]') : policy;
      return { data: { type: 'file', encoding: 'base64', size: Buffer.byteLength(text), content: Buffer.from(text).toString('base64') } };
    }, getCollaboratorPermissionLevel: async () => ({ data: { permission } }) },
  } };
  const context = { eventName, ref, sha: 'b'.repeat(40), payload: { repository: caller }, repo: { owner: 'caller', repo: 'repo' } };
  const environment = {
    TARGET_REPO: 'example/repo', TARGET_ITEM: '7', PROFILE: 'implement', TASK: 'Do it.',
    SKILLS: 'false', MAX_REQUESTS: '400', AGENT: 'opencode', AGENT_MODEL: 'p/m', AGENT_RUNNER: '"ubuntu-26.04"',
    INFERENCE_URL: 'http://proxy', INFERENCE_AUDIENCE: 'aud', INFERENCE_REGISTER: 'github-oidc', AGENT_CONFIG_REPO: 'o/c',
    BROKER_URL: 'http://broker', BROKER_AUDIENCE: 'broker-aud',
    AGENT_CONFIG_PATH: 'p', APPLY_ENVIRONMENT: 'apply', SESSION: 'false', IDLE_MINUTES: '',
    INFERENCE_PROVIDER: '', INFERENCE_API_KEY_SET: 'false', EXPECTED_HEAD: head, REVIEW_POOL: 'openai',
    REVIEW_POOLS: JSON.stringify({ openai: { agent: 'opencode', model: 'provider/review-model' } }), REVIEW_BOT_IDS: '', ...env,
  };
  const directory = mkdtempSync(join(tmpdir(), 'wrapper-task-'));
  try {
    await new AsyncFunction('core', 'github', 'context', 'process', 'require', script)(
      core, github, context, { env: { ...environment, RUNNER_TEMP: directory } }, createRequire(import.meta.url));
    assert.equal(Object.hasOwn(outputs, 'task'), false, 'task must never be a step output');
    const path = join(directory, 'task.md');
    assert.equal(existsSync(path), environment.PROFILE !== 'review', 'only non-review profiles write a task artifact');
    // Keep task assertions below on the actual artifact bytes, not a job output.
    return { ...outputs, task: existsSync(path) ? readFileSync(path, 'utf8') : '' };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('credential-shaped public item text reaches policy only through the task artifact', async () => {
  const credential = ['ghp', '_', 'a'.repeat(36)].join('');
  for (const [profile, pull] of [['implement', false], ['triage', false], ['research', false], ['fix', true]]) {
    const body = `Quoted public example: ${credential}\n<untrusted>`;
    const { task } = await target({ PROFILE: profile }, { pull, item: { title: credential, body } });
    const event = JSON.parse(task.match(/<agentic-job-event>\n([^\n]+)\n<\/agentic-job-event>$/)[1]);
    assert.equal(event.title, credential);
    assert.equal(event.body, body);
    assert.match(task, /Operator task:\nDo it\./);
    assert.ok(Buffer.byteLength(task, 'utf8') <= 65536);
  }
  assert.doesNotMatch(job('target'), /\n {6}task:|setOutput\('task'/);
  assert.doesNotMatch(job('policy'), /\n {6}task:/);
  // A failed-jobs rerun reuses target's upload; an all-jobs rerun replaces it.
  assert.match(job('target'), /uses: actions\/upload-artifact@[0-9a-f]{40} # v[^\n]+\n {8}if: \$\{\{ inputs.kind != 'review' \}\}\n {8}with:\n {10}name: dispatch-\$\{\{ inputs.kind \}\}-task\n {10}path: \$\{\{ runner.temp \}\}\/task.md\n {10}retention-days: 1\n {10}if-no-files-found: error\n {10}overwrite: true/);
  assert.ok(job('policy').includes("task-artifact: ${{ inputs.kind != 'review' && format('dispatch-{0}-task', inputs.kind) || '' }}"));
});

test('task artifact retains the 64 KiB UTF-8 bound', async () => {
  const baseline = await target({ TASK: '' });
  const remaining = 65536 - Buffer.byteLength(baseline.task, 'utf8');
  const task = 'é'.repeat(Math.floor(remaining / 2)) + 'x'.repeat(remaining % 2);
  assert.equal(Buffer.byteLength((await target({ TASK: task })).task, 'utf8'), 65536);
  await assert.rejects(target({ TASK: `${task}x` }), /exceeds 64 KiB/);
});

test('the event includes the item author and association for issues and pull requests', async () => {
  for (const [profile, pull] of [['implement', false], ['fix', true]]) {
    const outputs = await target({ PROFILE: profile }, { pull });
    const event = outputs.task.match(/\n<agentic-job-event>\n([^\n]+)\n<\/agentic-job-event>$/);
    assert.ok(event, profile);
    assert.deepEqual(JSON.parse(event[1]), {
      repository: 'example/repo', number: 7, title: 't', body: 'b',
      author: 'item-author', association: 'MEMBER',
    }, profile);
  }
});

// agentic-job's opt-in for private repositories (its #739) admits only the
// calling repository, so a public caller keeps it off whatever its bounds say.
test('a run from a repository that is not public fails before policy', async () => {
  for (const caller of [{ private: true, visibility: 'private' }, { private: false, visibility: 'internal' }, {}]) {
    await assert.rejects(target({}, { caller: { default_branch: 'main', ...caller } }), /only from a public repository/, JSON.stringify(caller));
  }
  assert.match(job('policy'), /\n {4}needs: target\n/);
});

test('skills are off by default and the task keeps its one-line contract', async () => {
  const outputs = await target({});
  assert.equal(outputs.skills, 'false');
  assert.doesNotMatch(outputs.task, /skill/);
  assert.match(outputs.task, /^Profile: implement\. Implement proposes one draft pull request\./);
});

test('artifact-only is default-off and admits only research without an apply environment', async () => {
  assert.match(workflow, /research-artifact-only:\n {8}description: [^\n]*\n {8}type: boolean\n {8}default: false/);
  assert.ok(job('target').includes('RESEARCH_ARTIFACT_ONLY: ${{ inputs.research-artifact-only }}'));
  for (const profile of ['implement', 'triage', 'session', 'fix', 'review']) {
    await assert.rejects(target({ PROFILE: profile, RESEARCH_ARTIFACT_ONLY: 'true' }), /requires kind research/);
  }
  await assert.rejects(target({ PROFILE: 'research', APPLY_ENVIRONMENT: '' }), /Missing the apply-environment/);
  const outputs = await target({ PROFILE: 'research', RESEARCH_ARTIFACT_ONLY: 'true', APPLY_ENVIRONMENT: '' });
  assert.match(outputs.task, /^Profile: research\./);
  for (const env of [{ SESSION: 'true' }, { IDLE_MINUTES: '10' }]) {
    await assert.rejects(target({ PROFILE: 'research', RESEARCH_ARTIFACT_ONLY: 'true', ...env }), /session and idle-minutes are refused/);
  }
});

test('triage-outputs-from-allow is default-off and admits only triage', async () => {
  assert.match(workflow, /triage-outputs-from-allow:\n {8}description: [^\n]*\n {8}type: boolean\n {8}default: false/);
  assert.ok(job('target').includes('TRIAGE_OUTPUTS_FROM_ALLOW: ${{ inputs.triage-outputs-from-allow }}'));
  for (const profile of ['implement', 'research', 'session', 'fix', 'review']) {
    await assert.rejects(target({ PROFILE: profile, TRIAGE_OUTPUTS_FROM_ALLOW: 'true' }), /requires kind triage/, profile);
  }
  const plain = await target({ PROFILE: 'triage' });
  assert.match(plain.task, /^Profile: triage\. Implement proposes one draft pull request\. Triage, research and session leave the tree clean and return one add_comment\./);
  const widened = await target({ PROFILE: 'triage', TRIAGE_OUTPUTS_FROM_ALLOW: 'true' });
  assert.match(widened.task, /^Profile: triage\. Triage leaves the tree clean and returns at most three of the outputs its bounds allow\.\n/);
});

test('hand-back releases original identity only after complete successful checked research', async () => {
  const handbackScript = job('research-handback').split('          script: |\n')[1]
    .split('\n').map(line => line.replace(/^ {12}/, '')).join('\n');
  const original = {
    target: { result: 'success', outputs: {} },
    policy: { result: 'success', outputs: { admitted: 'true' } },
    agent: { result: 'success', outputs: { exit: '0', job: '123',
      'safe-outputs-artifact-id': '456', 'safe-outputs-artifact-digest': 'a'.repeat(64) } },
    check: { result: 'success', outputs: { 'artifact-id': '789', 'artifact-digest': 'b'.repeat(64), 'has-patch': 'false' } },
  };
  const run = async needs => {
    const outputs = {};
    let summary;
    const core = { setOutput: (k, v) => { outputs[k] = v; }, summary: {
      addRaw: value => { summary = value; return { write: async () => {} }; },
    } };
    await new AsyncFunction('core', 'context', 'process', handbackScript)(core,
      { repo: { owner: 'caller', repo: 'repo' } },
      { env: { NEEDS: JSON.stringify(needs), GITHUB_RUN_ID: '12', GITHUB_RUN_ATTEMPT: '2' } });
    assert.match(summary, /No proposals applied/);
    return outputs;
  };
  assert.deepEqual(await run(original), { status: 'ready', 'artifact-id': '456', 'artifact-digest': 'a'.repeat(64),
    repository: 'caller/repo', 'run-id': '12', 'run-attempt': '2', 'agent-job': '123' });
  const refuse = async mutate => {
    const needs = structuredClone(original);
    mutate(needs);
    assert.deepEqual(await run(needs), { status: 'unavailable' });
  };
  for (const name of ['target', 'policy', 'agent', 'check']) {
    for (const result of ['failure', 'cancelled', 'skipped']) await refuse(n => { n[name].result = result; });
  }
  for (const exit of ['', '3', '124', '1']) await refuse(n => { n.agent.outputs.exit = exit; });
  await refuse(n => { n.agent.outputs['stopped-early'] = 'true'; });
  await refuse(n => { n.policy.outputs.admitted = 'false'; });
  for (const value of ['', 'true']) await refuse(n => { n.check.outputs['has-patch'] = value; });
  for (const [name, fields] of [['agent', ['safe-outputs-artifact-id', 'safe-outputs-artifact-digest', 'job']],
    ['check', ['artifact-id', 'artifact-digest']]]) {
    for (const field of fields) {
      for (const value of ['', undefined, 'unchecked-name', '1\ninjected=true']) {
        await refuse(n => { n[name].outputs[field] = value; });
      }
    }
  }
});

test('skills turn on only for implement or fix with claude or opencode', async () => {
  for (const agent of ['claude', 'opencode']) {
    const outputs = await target({ SKILLS: 'true', AGENT: agent });
    assert.equal(outputs.skills, 'true');
    assert.match(outputs.task, /^Profile: implement\. Use the implement skill\. /);
  }
  // agentic-job refuses skills for these agents: the input must not fail
  // their runs.
  for (const agent of ['vibe']) {
    const outputs = await target({ SKILLS: 'true', AGENT: agent, AGENT_MODEL: 'devstral-small-latest' });
    assert.equal(outputs.skills, 'false');
    assert.doesNotMatch(outputs.task, /skill/);
  }
  // Fix installs them for the commit-messages skill but names no procedure.
  const fix = await target({ SKILLS: 'true', PROFILE: 'fix' }, { pull: true });
  assert.equal(fix.skills, 'true');
  assert.doesNotMatch(fix.task, /skill/);
  for (const [profile, pull] of [['triage', false], ['research', false]]) {
    const outputs = await target({ SKILLS: 'true', PROFILE: profile }, { pull });
    assert.equal(outputs.skills, 'false', profile);
    assert.doesNotMatch(outputs.task, /skill/, profile);
  }
});

test('max-requests defaults to 400 and passes through for a real agent', async () => {
  assert.match(workflow, /\n {6}max-requests:\n {8}description: [^\n]*\n {8}type: string\n {8}default: "400"\n/);
  assert.equal((await target({}))['max-requests'], '400');
  assert.equal((await target({ MAX_REQUESTS: '1000', AGENT: 'claude' }))['max-requests'], '1000');
  assert.equal((await target({ MAX_REQUESTS: '1', AGENT: 'vibe', AGENT_MODEL: 'devstral-small-latest' }))['max-requests'], '1');
  await assert.rejects(target({ AGENT: 'fake' }), /fake is unsupported by this wrapper/);
});

test('max-requests outside 1 to 1000 fails the run', async () => {
  for (const value of ['', '0', '1001', '-1', '01', '4e2', ' 400', '400 ']) {
    await assert.rejects(target({ MAX_REQUESTS: value }), /max-requests must be/, JSON.stringify(value));
  }
});

// The text of one job, from its key to the next job's.
function job(name) {
  return `\n${workflow.split(`\n  ${name}:\n`)[1].split(/\n {2}[a-z-]+:\n/)[0]}`;
}

// The pinned upstream rejects session.listen during runner admission and
// run. Every profile must fail here, before policy or registration.
test('sessions are refused before provisioning until the upstream pin supports them', async () => {
  for (const profile of ['implement', 'fix', 'triage', 'research', 'review', 'session']) {
    for (const env of [{ SESSION: 'true' }, { SESSION: 'true', IDLE_MINUTES: '60' }, { IDLE_MINUTES: '10' }, { IDLE_MINUTES: '0' }]) {
      await assert.rejects(target({ ...env, PROFILE: profile }, { pull: true, allowPolicyRead: false }),
        /session and idle-minutes are refused: the pinned agentic-job/, `${profile}: ${JSON.stringify(env)}`);
    }
  }
  const outputs = await target({});
  for (const name of ['session', 'idle-minutes', 'session-host', 'session-endpoint']) {
    assert.equal(Object.hasOwn(outputs, name), false, name);
  }
});

// A provider key needed the sandbox user: with the runner as the only
// agent user (agentic-job#2989) the agent can read whatever its job holds.
// Refused before anything is provisioned, for every profile; the inputs
// stay so a caller passing one gets this.
test('provider-key mode is refused before provisioning', async () => {
  const refusals = [
    [{ INFERENCE_REGISTER: 'api-key', AGENT: 'claude' }, /inference-register must be github-oidc/],
    [{ INFERENCE_REGISTER: '' }, /inference-register must be github-oidc/],
    [{ INFERENCE_PROVIDER: 'anthropic' }, /inference-register must be github-oidc/],
    [{ INFERENCE_API_KEY_SET: 'true' }, /inference-register must be github-oidc/],
    [{ INFERENCE_API_KEY_SET: undefined }, /inference-register must be github-oidc/],
  ];
  for (const [env, error] of refusals) {
    await assert.rejects(target(env, { allowPolicyRead: false }), error, JSON.stringify(env));
    await assert.rejects(review(env, { pull: true, allowPolicyRead: false }), error, JSON.stringify(env));
  }
});

// A repository-triage session (tracker#1505) is a comment-only analysis
// run on an issue, with triage's checks; the listener input stays refused.
test('kind session is admitted on an issue as a comment-only run', async () => {
  const outputs = await target({ PROFILE: 'session', SKILLS: 'true', AGENT: 'claude' });
  assert.equal(outputs.skills, 'false');
  assert.equal(outputs['max-requests'], '400');
  assert.match(outputs.task, /^Profile: session\. [^\n]*Triage, research and session leave the tree clean and return one add_comment\./);
  await assert.rejects(target({ PROFILE: 'session' }, { pull: true }), /requires an issue, not a pull request/);
  await assert.rejects(target({ PROFILE: 'session', SESSION: 'true' }), /session and idle-minutes are refused/);
  await assert.rejects(target({ PROFILE: 'session', IDLE_MINUTES: '10' }), /session and idle-minutes are refused/);
  await assert.rejects(target({ PROFILE: 'session', AGENT: 'fake' }), /fake is unsupported/);
  await assert.rejects(target({ PROFILE: 'session', MAX_REQUESTS: '1001' }), /max-requests must be/);
  await assert.rejects(target({ PROFILE: 'session', APPLY_ENVIRONMENT: '' }), /Missing the apply-environment/);
  for (const profile of ['Session', 'sessions', ' session', '']) {
    await assert.rejects(target({ PROFILE: profile }), /Unknown dispatch profile/, JSON.stringify(profile));
  }
});

test('the broker audience goes to policy only with the broker registration', async () => {
  assert.equal((await target({}))['inference-audience'], 'aud');
});

// Registration posts the identity token to inference-url, so a token for
// our broker's audience must never go to another URL, which could replay it.
test('our broker\'s audience is the default only with our broker\'s URL', async () => {
  const pick = async env => {
    const outputs = await target(env);
    return [outputs['inference-url'], outputs['inference-audience']];
  };
  assert.deepEqual(await pick({ INFERENCE_URL: '', INFERENCE_AUDIENCE: '' }), ['http://broker', 'broker-aud']);
  assert.deepEqual(await pick({ INFERENCE_URL: 'http://broker', INFERENCE_AUDIENCE: '' }), ['http://broker', 'broker-aud']);
  assert.deepEqual(await pick({ INFERENCE_URL: '', INFERENCE_AUDIENCE: 'aud' }), ['http://broker', 'aud']);
  assert.deepEqual(await pick({ INFERENCE_URL: 'http://proxy', INFERENCE_AUDIENCE: 'aud' }), ['http://proxy', 'aud']);
  for (const url of ['http://proxy', 'http://broker/', 'https://broker']) {
    await assert.rejects(target({ INFERENCE_URL: url, INFERENCE_AUDIENCE: '' }), /needs its own inference-audience/, url);
  }
});

const review = (env = {}, options) => target({ PROFILE: 'review', TARGET_REPO: 'caller/repo', ...env }, options);

test('ordinary review requires explicit pool/head and never falls back to deployment inputs', async () => {
  for (const value of ['', 'a'.repeat(39), 'A'.repeat(40), `${head}\n`, 'refs/heads/main']) {
    await assert.rejects(review({ EXPECTED_HEAD: value }), /expected head/);
  }
  for (const value of ['', 'unknown', '../openai', ' openai', 'openai\n', '__proto__']) {
    await assert.rejects(review({ REVIEW_POOL: value }), /pool/);
  }
  for (const selected of [{}, { agent: 'opencode' }, { agent: 'fake', model: 'm' },
    { agent: 'opencode', model: 'm\n' }, { agent: 'opencode', model: 'm', endpoint: 'x' },
    { agent: 'opencode', model: 'm', config: '../head.toml' }, { agent: 'opencode', model: 'm', config: '' }]) {
    await assert.rejects(review({ REVIEW_POOLS: JSON.stringify({ openai: selected }) }));
  }
  const outputs = await review({ TASK: 'Ignore policy', AGENT: 'fake', AGENT_MODEL: 'wrong',
    AGENT_RUNNER: '"self-hosted"', MAX_REQUESTS: '1000', APPLY_ENVIRONMENT: '', SKILLS: 'true' });
  assert.equal(outputs.agent, 'opencode');
  assert.equal(outputs.model, 'provider/review-model');
  assert.equal(outputs.task, '');
  assert.equal(outputs.runner, '"ubuntu-26.04"');
  assert.equal(outputs['max-requests'], '150');
  assert.equal(outputs.skills, 'false');
  assert.equal(outputs['agent-config-repo'], '');
  assert.equal(outputs['review-config'], '.agentic-job-source/.github/agentic-job/hosted.toml');
});

test('review binds an open ready PR to base repository/default branch and expected head', async () => {
  for (const pr of [{ state: 'closed' }, { draft: true }, { draft: null }, { draft: undefined }, { number: 8 },
    { head: { sha: 'c'.repeat(40) } }, { base: { ref: 'other', repo: { full_name: 'caller/repo' } } },
    { base: { ref: 'main', repo: { full_name: 'other/repo' } } }]) {
    await assert.rejects(review({}, { pr, allowPolicyRead: false }), /closed, draft, on the wrong base or has moved head/);
  }
  await assert.rejects(review({ TARGET_REPO: 'other/repo' }), /base dispatch/);
  await assert.rejects(review({}, { eventName: 'pull_request_target' }), /base dispatch/);
  await assert.rejects(review({}, { ref: 'refs/heads/untrusted' }), /default branch/);
  for (const item of ['', '0', '01', '1e2', '9007199254740992']) await assert.rejects(review({ TARGET_ITEM: item }), /positive item/);
});

test('review admission keeps association, immutable bot id AND base bounds, and writer command checks', async () => {
  const outsider = { author_association: 'CONTRIBUTOR' };
  await assert.rejects(review({}, { pr: outsider }), /not admitted/);
  const bot = { ...outsider, user: { type: 'Bot', id: 123, login: 'dependabot[bot]' } };
  await assert.rejects(review({}, { pr: bot }), /not admitted/);
  await review({ REVIEW_BOT_IDS: '[123]' }, { pr: bot });
  await assert.rejects(review({ REVIEW_BOT_IDS: '[123]' }, { pr: bot, policy: basePolicy.replace('dependabot[bot]', 'other[bot]') }), /not admitted/);
  const associatedBot = { ...bot, author_association: 'MEMBER' };
  await assert.rejects(review({}, { pr: associatedBot }), /not admitted/);
  await assert.rejects(review({ REVIEW_BOT_IDS: '[123]' }, { pr: associatedBot, policy: basePolicy.replace('dependabot[bot]', 'other[bot]') }), /not admitted/);
  await review({ REVIEW_BOT_IDS: '[123]' }, { pr: associatedBot });
  const roles = basePolicy.replace('["all"]', '["write", "maintain", "admin"]');
  await review({}, { policy: roles, permission: 'write' });
  await assert.rejects(review({}, { policy: roles, permission: 'read' }), /not admitted/);
  for (const policy of [basePolicy.replace('caller/repo', 'other/repo'), basePolicy.replace('["main"]', '["other"]'),
    basePolicy.replace('forks = true', 'forks = false'), basePolicy.concat('\nroles = ["all"]'),
    basePolicy.concat('\nunknown = true')]) await assert.rejects(review({}, { policy }));
  const comments = [{ body: '/review', user: { type: 'User', login: 'writer' } }];
  await review({}, { pr: outsider, comments });
  const commandPolicy = basePolicy.replace('["pull_request_target"]', '["issue_comment"]').replace('["all"]', '["write"]').concat('\ncommands = ["/review"]');
  const commandReview = await review({}, { policy: basePolicy.replace('forks = true', 'forks = false'), commandPolicy, comments });
  assert.equal(commandReview['review-allow'], 'workflow/review-command.toml');
  assert.equal((await review())['review-allow'], 'workflow/review.toml');
  for (const permission of ['read', 'triage', '']) await assert.rejects(review({}, { pr: outsider, comments, permission }), /not admitted/);
  await assert.rejects(review({}, { pr: outsider, comments: Array(100).fill(comments[0]) }), /history exceeds/);
});

test('submission preflight rejects a moved head, closure, draft or changed base', async () => {
  const preflight = job('submit').split('          script: |\n')[1].split('\n      - name:')[0]
    .split('\n').map(line => line.replace(/^ {12}/, '')).join('\n');
  const original = { state: 'open', draft: false, head: { sha: head }, base: { ref: 'main', repo: { full_name: 'caller/repo' } } };
  const run = pull => new AsyncFunction('github', 'context', 'process', preflight)(
    { rest: { pulls: { get: async () => ({ data: pull }) } } }, { repo: { owner: 'caller', repo: 'repo' } },
    { env: { PULL: '7', HEAD_SHA: head, BASE: 'main' } });
  await run(original);
  for (const delta of [{ head: { sha: 'c'.repeat(40) } }, { state: 'closed' }, { draft: true }, { draft: null },
    { base: { ref: 'other', repo: { full_name: 'caller/repo' } } },
    { base: { ref: 'main', repo: { full_name: 'other/repo' } } }]) await assert.rejects(run({ ...original, ...delta }), /before publication/);
});
