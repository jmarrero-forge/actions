import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

// The wrappers' tests that execute no JavaScript are in lint/tests/wrappers.rs.
const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const workflow = readFileSync(new URL('./cgwalters-agentic-job.yml', import.meta.url), 'utf8');

// A repository without INFERENCE_URL falls back to the broker its runner
// joins the tailnet to reach: the wrappers and tailnets.json move together.
test('the wrappers fall back to the cgwalters-forge tailnet broker', () => {
  const tailnets = JSON.parse(readFileSync(new URL('../../tailscale/tailnets.json', import.meta.url), 'utf8'));
  const broker = tailnets['cgwalters-forge'].proxy;
  const review = readFileSync(new URL('./cgwalters-review.yml', import.meta.url), 'utf8');
  const fallbacks = [...review.matchAll(/vars\.INFERENCE_URL \|\| '([^']*)'/g)].map(match => match[1]);
  assert.deepEqual(fallbacks, [broker]);
  assert.deepEqual([...workflow.matchAll(/\n {10}BROKER_URL: (\S+)\n/g)].map(match => match[1]), [broker]);
});

// The review wrapper has no inputs and its caller passes no secret, so it
// stays on the broker: agentic-job's provider-key mode (its #740) is not
// wired here.
test('the review wrapper stays on the inference broker', () => {
  assert.doesNotMatch(read('./cgwalters-review.yml'), /INFERENCE_API_KEY|inference-register|inference-provider|inference-api-key/);
});

// Renovate's matchStrings are JavaScript regular expressions, so this one
// runs them here rather than in lint/tests/wrappers.rs.
test('Renovate tracks all three review pins from main, without automerge or dispatch updates', () => {
  const config = JSON.parse(read('../renovate.json'));
  assert.equal(config.customManagers.length, 1);
  const manager = config.customManagers[0];
  assert.equal(manager.customType, 'regex');
  assert.deepEqual(manager.managerFilePatterns, ['/^\\.github/workflows/cgwalters-review\\.yml$/']);
  assert.equal(manager.datasourceTemplate, 'git-refs');
  assert.equal(manager.packageNameTemplate, 'https://github.com/cgwalters-forge/actions');
  assert.equal(manager.currentValueTemplate, 'main');
  const pattern = new RegExp(manager.matchStrings[0], 'g');
  const wrapper = read('./cgwalters-review.yml');
  const pins = [...wrapper.matchAll(pattern)];
  assert.equal(pins.length, 3);
  for (const pin of pins) assert.match(pin.groups.currentDigest, /^[0-9a-f]{40}$/);
  // Exercise the digest-only replacement: preserve the action paths and branch.
  const updated = wrapper.replace(pattern, match => match.replace(/[0-9a-f]{40}/, 'a'.repeat(40)));
  assert.equal([...updated.matchAll(pattern)].filter(pin => pin.groups.currentDigest === 'a'.repeat(40)).length, 3);
  assert.equal([...read('./cgwalters-agentic-job.yml').matchAll(pattern)].length, 0);
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
const script = workflow.split('          script: |\n')[1].split('\n\n')[0]
  .split('\n').map(line => line.replace(/^ {12}/, '')).join('\n');
const AsyncFunction = (async () => {}).constructor;

async function target(env, { pull = false, caller = { private: false, visibility: 'public' } } = {}) {
  const outputs = {};
  const core = { setOutput: (name, value) => { outputs[name] = value; } };
  const github = { rest: {
    issues: { get: async ({ issue_number }) => ({
      data: { number: issue_number, title: 't', body: 'b', user: { login: 'item-author' }, author_association: 'MEMBER', ...(pull ? { pull_request: {} } : {}) },
    }) },
    repos: { get: async ({ owner, repo }) => {
      assert.equal(`${owner}/${repo}`, 'caller/repo');
      return { data: caller };
    } },
  } };
  const context = { eventName: 'workflow_call', repo: { owner: 'caller', repo: 'repo' } };
  const environment = {
    TARGET_REPO: 'example/repo', TARGET_ITEM: '7', PROFILE: 'implement', TASK: 'Do it.',
    SKILLS: 'false', MAX_REQUESTS: '400', AGENT: 'opencode', AGENT_MODEL: 'p/m', AGENT_RUNNER: '"ubuntu-26.04"',
    INFERENCE_URL: 'http://proxy', INFERENCE_AUDIENCE: 'aud', INFERENCE_REGISTER: 'github-oidc', AGENT_CONFIG_REPO: 'o/c',
    BROKER_URL: 'http://broker', BROKER_AUDIENCE: 'broker-aud',
    AGENT_CONFIG_PATH: 'p', APPLY_ENVIRONMENT: 'apply', SESSION: 'false', IDLE_MINUTES: '',
    TAILNET_DOMAIN: '', RUN_ID: '123', RUN_ATTEMPT: '2', ...env,
  };
  await new AsyncFunction('core', 'github', 'context', 'process', script)(
    core, github, context, { env: environment });
  return outputs;
}

test('the event includes the item author and association for issues and pull requests', async () => {
  for (const [profile, pull] of [['implement', false], ['review', true], ['fix', true]]) {
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
    await assert.rejects(target({}, { caller }), /only from a public repository/, JSON.stringify(caller));
  }
  assert.match(job('policy'), /\n {4}needs: target\n/);
});

test('skills are off by default and the task keeps its one-line contract', async () => {
  const outputs = await target({});
  assert.equal(outputs.skills, 'false');
  assert.doesNotMatch(outputs.task, /skill/);
  assert.match(outputs.task, /^Profile: implement\. Implement proposes one draft pull request\./);
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
  for (const [profile, pull] of [['triage', false], ['research', false], ['review', true]]) {
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
  await assert.rejects(target({ AGENT: 'fake' }), /fake is unsupported with agent-user sandbox/);
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

test('a run is no session unless asked, and passes policy nothing new', async () => {
  assert.match(workflow, /\n {6}session:\n {8}description: [^\n]*\n {8}type: boolean\n {8}default: false\n/);
  assert.match(workflow, /\n {6}idle-minutes:\n {8}description: [^\n]*\n {8}type: string\n {8}default: ""\n/);
  const outputs = await target({ TAILNET_DOMAIN: 'example.ts.net' });
  assert.equal(outputs.session, 'false');
  assert.equal(outputs['idle-minutes'], '');
  assert.equal(outputs['session-endpoint'], '');
  assert.match(workflow, /\n {6}session: \$\{\{ needs\.target\.outputs\.session == 'true' \}\}\n/);
  assert.match(workflow, /\n {6}idle-minutes: \$\{\{ needs\.target\.outputs\.idle-minutes \}\}\n/);
  assert.match(workflow, /\n {10}SESSION: \$\{\{ inputs\.session \}\}\n {10}IDLE_MINUTES: \$\{\{ inputs\.idle-minutes \}\}\n/);
});

test('a session names its endpoint from the join hostname and the repository variable', async () => {
  for (const [agent, idle] of [['opencode', '60'], ['claude', '1'], ['claude', '']]) {
    const outputs = await target({ SESSION: 'true', IDLE_MINUTES: idle, TAILNET_DOMAIN: 'tail-1.ts.net', AGENT: agent });
    assert.equal(outputs.session, 'true');
    assert.equal(outputs['idle-minutes'], idle);
    assert.equal(outputs['session-host'], 'agentic-job-123-2');
    assert.equal(outputs['session-endpoint'], 'wss://agentic-job-123-2.tail-1.ts.net/');
  }
  assert.equal((await target({}))['session-host'], '');
  // The address is the node the agent job joins as, also when that job
  // alone is re-run, on a domain only the calling repository sets.
  assert.match(job('agent'), /\n {10}hostname: \$\{\{ needs\.target\.outputs\.session-host \|\| format\('agentic-job-\{0\}-\{1\}', github\.run_id, github\.run_attempt\) \}\}\n/);
  assert.match(job('target'), /\n {6}session-host: \$\{\{ steps\.target\.outputs\.session-host \}\}\n/);
  assert.match(workflow, /\n {10}TAILNET_DOMAIN: \$\{\{ vars\.TAILNET_DOMAIN \}\}\n/);
  assert.doesNotMatch(workflow, /inputs\.tailnet/i);
});

test('a session without a usable domain, for review, or with idle-minutes out of bounds fails', async () => {
  for (const domain of ['', 'localhost', 'Example.ts.net', 'a.ts.net/x', 'a.ts.net`', '-a.ts.net', 'a.ts.net\n']) {
    await assert.rejects(target({ SESSION: 'true', TAILNET_DOMAIN: domain }), /TAILNET_DOMAIN/, JSON.stringify(domain));
  }
  await assert.rejects(target({ SESSION: 'true', TAILNET_DOMAIN: 'a.ts.net', PROFILE: 'review' }, { pull: true }), /review/);
  for (const value of ['0', '61', '100', '01', ' 5', '5 ', '1e1', '-1']) {
    await assert.rejects(target({ SESSION: 'true', TAILNET_DOMAIN: 'a.ts.net', IDLE_MINUTES: value }), /idle-minutes/, JSON.stringify(value));
  }
  // Without session it would be ignored downstream; refuse it instead.
  await assert.rejects(target({ IDLE_MINUTES: '10' }), /idle-minutes/);
});

// agentic-job's provider-key mode (its #740), off unless a caller asks:
// the defaults are its broker's, and the key, which `run` holds outside
// the agent's sandbox, is handed to that one step and to nothing else.
test('provider-key mode passes through with broker defaults, the key to run alone', () => {
  assert.match(workflow, /\n {6}inference-register:\n {8}description: [^\n]*\n {8}type: string\n {8}default: github-oidc\n/);
  assert.match(workflow, /\n {6}inference-provider:\n {8}description: [^\n]*\n {8}type: string\n {8}default: ""\n/);
  assert.match(workflow, /\n {6}INFERENCE_API_KEY:\n {8}required: false\n/);
  assert.match(job('policy'), /\n {6}inference-register: \$\{\{ inputs\.inference-register \}\}\n {6}inference-provider: \$\{\{ inputs\.inference-provider \}\}\n/);
  const uses = [...workflow.matchAll(/secrets\.INFERENCE_API_KEY/g)];
  assert.equal(uses.length, 1);
  assert.match(job('agent'), /\n {8}uses: \.\/\.agentic-job-source\/run\n {8}with:\n {10}job-dir: \/var\/tmp\/agentic-job\n {10}agent-user: sandbox\n {10}github-token: \$\{\{ github\.token \}\}\n {10}inference-api-key: \$\{\{ secrets\.INFERENCE_API_KEY \}\}\n/);
});

test('the broker audience goes to policy only with the broker registration', async () => {
  assert.equal((await target({}))['inference-audience'], 'aud');
  assert.equal((await target({ INFERENCE_REGISTER: 'api-key', AGENT: 'claude' }))['inference-audience'], '');
  assert.match(workflow, /\n {10}INFERENCE_REGISTER: \$\{\{ inputs\.inference-register \}\}\n/);
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
  // No identity token is minted for the proxy otherwise: nothing to refuse.
  assert.deepEqual(await pick({ INFERENCE_URL: 'https://api.example', INFERENCE_AUDIENCE: '', INFERENCE_REGISTER: 'api-key', AGENT: 'claude' }), ['https://api.example', '']);
  // Neither has a default of its own in the expressions, which would set
  // one without the other.
  assert.match(workflow, /\n {10}INFERENCE_URL: \$\{\{ inputs\.inference-url \|\| vars\.INFERENCE_URL \}\}\n/);
  assert.match(workflow, /\n {10}INFERENCE_AUDIENCE: \$\{\{ inputs\.inference-audience \|\| vars\.INFERENCE_AUDIENCE \}\}\n/);
});
