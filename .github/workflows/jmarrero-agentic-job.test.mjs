import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const workflow = readFileSync(new URL('./jmarrero-agentic-job.yml', import.meta.url), 'utf8');

// agentic-job is called at one release, by its commit: check, apply,
// prepare and run refuse calls that resolved to different commits, which
// following main produced for runs spanning an agentic-job merge
// (cgwalters-forge/tracker#544), and a tag can be moved. Our own tailscale
// action, which holds the tailnet join's credentials, comes from the commit
// this file runs at, so nothing pins a commit of this repository.
test('agentic-job is called at one release commit, our tailscale at this file\'s', () => {
  const refs = [...workflow.matchAll(/uses: ([a-z-]+-forge\/\S+)@(\S+)/g)];
  assert.deepEqual(refs.map(match => match[1]), [
    'cgwalters-forge/agentic-job/.github/workflows/policy.yml',
    'cgwalters-forge/agentic-job/.github/workflows/check.yml',
    'cgwalters-forge/agentic-job/.github/workflows/apply.yml',
  ]);
  const agent = job('agent');
  assert.match(agent, /\n {10}WORKFLOW_REPOSITORY: \$\{\{ job\.workflow_repository \}\}\n {10}WORKFLOW_SHA: \$\{\{ job\.workflow_sha \}\}\n {10}EXPECTED_REPOSITORY: jmarrero-forge\/actions\n/);
  assert.match(agent, /\n {10}repository: \$\{\{ job\.workflow_repository \}\}\n {10}ref: \$\{\{ job\.workflow_sha \}\}\n {10}path: \.jmarrero-actions\n {10}sparse-checkout: tailscale\n {10}persist-credentials: false\n/);
  assert.match(agent, /- name: Join the tailnet\n[^\n]*\n {8}uses: \.\/\.jmarrero-actions\/tailscale\n/);
  for (const [, path, ref] of refs) assert.match(ref, /^[0-9a-f]{40}$/, path);
  const calls = [...workflow.matchAll(/uses: cgwalters-forge\/agentic-job\/\S+@([0-9a-f]{40}) # (v\d+\.\d+\.\d+)\n/g)];
  assert.equal(calls.length, 3);
  assert.equal(new Set(calls.map(match => `${match[1]} ${match[2]}`)).size, 1);
  assert.doesNotMatch(workflow, /PINNED_SHA/);
  assert.match(workflow, /\n {10}EXPECTED_REPOSITORY: cgwalters-forge\/agentic-job\n/);
});

// Nothing falls back to cgwalters' deployment: his broker, its audience,
// his model or his opencode configuration.
test('no default names another deployment', () => {
  assert.doesNotMatch(workflow, /100\.121\.|praxis|cgwalters-bot|uses: cgwalters-forge\/actions|AGENT_CONFIG_REPO|AGENT_CONFIG_PATH/);
  assert.match(workflow, /\n {10}AGENT: \$\{\{ inputs\.agent \|\| vars\.AGENT \|\| 'claude' \}\}\n/);
  assert.match(workflow, /\n {10}AGENT_MODEL: \$\{\{ inputs\.model \|\| vars\.AGENT_MODEL \}\}\n/);
  assert.match(workflow, /\n {10}INFERENCE_URL: \$\{\{ inputs\.inference-url \|\| vars\.INFERENCE_URL \}\}\n/);
  assert.match(workflow, /\n {10}INFERENCE_AUDIENCE: \$\{\{ inputs\.inference-audience \|\| vars\.INFERENCE_AUDIENCE \}\}\n/);
});

// Dependabot moves the commit, all three calls in one pull request, after
// the same cooldown as any other third-party action.
test('dependabot proposes new agentic-job releases as one group', () => {
  const config = readFileSync(new URL('../dependabot.yml', import.meta.url), 'utf8');
  assert.match(config, /\n {6}interval: daily\n/);
  assert.match(config, /\n {6}agentic-job:\n {8}patterns: \["cgwalters-forge\/agentic-job\*"\]\n/);
  assert.match(config, /\n {6}default-days: 7\n/);
  assert.doesNotMatch(config, /exclude:/);
});

// Every other action is pinned by full commit too; dependabot moves them.
test('every other action is pinned by full commit', () => {
  for (const file of ['./jmarrero-agentic-job.yml', './ci.yml', '../../tailscale/action.yml']) {
    const text = readFileSync(new URL(file, import.meta.url), 'utf8');
    const others = [...text.matchAll(/uses: (?!\.\/)(\S+)@(\S+)/g)];
    assert.ok(others.length > 0, file);
    for (const [, name, ref] of others) assert.match(ref, /^[0-9a-f]{40}$/, `${file}: ${name}`);
  }
});

// The target job's actual script, with doubles for github-script's globals.
const script = workflow.split('          script: |\n')[1].split('\n\n')[0]
  .split('\n').map(line => line.replace(/^ {12}/, '')).join('\n');
const AsyncFunction = (async () => {}).constructor;

async function target(env, { pull = false } = {}) {
  const outputs = {};
  const core = { setOutput: (name, value) => { outputs[name] = value; } };
  const github = { rest: { issues: { get: async ({ issue_number }) => ({
    data: { number: issue_number, title: 't', body: 'b', ...(pull ? { pull_request: {} } : {}) },
  }) } } };
  const context = { eventName: 'workflow_call' };
  const environment = {
    TARGET_REPO: 'example/repo', TARGET_ITEM: '7', PROFILE: 'implement', TASK: 'Do it.',
    SKILLS: 'false', MAX_REQUESTS: '400', AGENT: 'claude', AGENT_MODEL: 'm', AGENT_RUNNER: '"ubuntu-26.04"',
    INFERENCE_URL: 'http://proxy', INFERENCE_AUDIENCE: 'aud',
    APPLY_ENVIRONMENT: 'apply', SESSION: 'false', IDLE_MINUTES: '',
    TAILNET_DOMAIN: '', RUN_ID: '123', RUN_ATTEMPT: '2', ...env,
  };
  await new AsyncFunction('core', 'github', 'context', 'process', script)(
    core, github, context, { env: environment });
  return outputs;
}

test('skills are off by default and the task keeps its one-line contract', async () => {
  const outputs = await target({});
  assert.equal(outputs.skills, 'false');
  assert.doesNotMatch(outputs.task, /skill/);
  assert.match(outputs.task, /^Profile: implement\. Implement proposes one draft pull request\./);
});

test('skills turn on only for implement with claude', async () => {
  const outputs = await target({ SKILLS: 'true' });
  assert.equal(outputs.skills, 'true');
  assert.match(outputs.task, /^Profile: implement\. Use the implement skill\. /);
  // agentic-job refuses skills for the scripted agent: the input must not
  // fail its runs.
  const fake = await target({ SKILLS: 'true', AGENT: 'fake' });
  assert.equal(fake.skills, 'false');
  assert.doesNotMatch(fake.task, /skill/);
  for (const [profile, pull] of [['triage', false], ['research', false], ['review', true], ['fix', true]]) {
    const outputs = await target({ SKILLS: 'true', PROFILE: profile }, { pull });
    assert.equal(outputs.skills, 'false', profile);
    assert.doesNotMatch(outputs.task, /skill/, profile);
  }
});

test('policy is given skills only from the target job', () => {
  assert.match(workflow, /\n {10}SKILLS: \$\{\{ inputs\.skills \}\}\n/);
  assert.match(workflow, /\n {6}agent-skills: \$\{\{ needs\.target\.outputs\.skills == 'true' \}\}\n/);
  assert.match(workflow, /\n {6}skills: \$\{\{ steps\.target\.outputs\.skills \}\}\n/);
});

test('max-requests defaults to 400, passes through for a real agent and is 0 for fake', async () => {
  assert.match(workflow, /\n {6}max-requests:\n {8}description: [^\n]*\n {8}type: string\n {8}default: "400"\n/);
  assert.equal((await target({}))['max-requests'], '400');
  assert.equal((await target({ MAX_REQUESTS: '1000' }))['max-requests'], '1000');
  assert.equal((await target({ MAX_REQUESTS: '1' }))['max-requests'], '1');
  assert.equal((await target({ MAX_REQUESTS: '900', AGENT: 'fake' }))['max-requests'], '0');
});

test('max-requests outside 1 to 1000 fails the run', async () => {
  for (const value of ['', '0', '1001', '-1', '01', '4e2', ' 400', '400 ']) {
    await assert.rejects(target({ MAX_REQUESTS: value }), /max-requests must be/, JSON.stringify(value));
  }
});

test('policy is given max-requests only from the target job', () => {
  assert.match(workflow, /\n {10}MAX_REQUESTS: \$\{\{ inputs\.max-requests \}\}\n/);
  assert.match(workflow, /\n {6}max-requests: \$\{\{ needs\.target\.outputs\.max-requests \}\}\n/);
  assert.match(workflow, /\n {6}max-requests: \$\{\{ steps\.target\.outputs\.max-requests \}\}\n/);
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
  for (const [agent, idle] of [['claude', '60'], ['fake', '1'], ['claude', '']]) {
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

test('the tailnet join is for a real agent or a session', () => {
  assert.match(job('agent'), /- name: Join the tailnet\n {8}if: \$\{\{ needs\.target\.outputs\.agent != 'fake' \|\| needs\.target\.outputs\.session == 'true' \}\}\n/);
  // So are the check and checkout of this repository that the join comes from.
  assert.equal(job('agent').split("if: ${{ needs.target.outputs.agent != 'fake' || needs.target.outputs.session == 'true' }}").length - 1, 3);
});

test('the session job posts the endpoint before apply concludes, and only for a session', () => {
  const session = job('session');
  assert.match(session, /\n {4}needs: \[target, policy\]\n/);
  assert.match(session, /\n {4}if: \$\{\{ needs\.policy\.outputs\.admitted == 'true' && needs\.target\.outputs\.session == 'true' && needs\.policy\.outputs\.notified == 'true' && needs\.policy\.outputs\.notify == 'comment' \}\}\n/);
  assert.match(session, /\n {4}concurrency:\n {6}group: agentic-job-status-\$\{\{ needs\.policy\.outputs\.notify-repo \}\}-\$\{\{ needs\.policy\.outputs\.notify-item \}\}\n {6}cancel-in-progress: false\n/);
  assert.match(session, /\n {4}runs-on: ubuntu-26\.04\n/);
  assert.match(session, /\n {4}continue-on-error: true\n/);
  assert.match(session, /\n {10}EXPECTED_REPOSITORY: cgwalters-forge\/agentic-job\n/);
  assert.match(session, /`\$\{\{ needs\.target\.outputs\.session-endpoint \}\}`/);
  assert.match(session, /\n {8}run: node \.agentic-job-source\/workflow\/status\.cjs\n/);
  // Nothing of the agent's: no artifact is downloaded here.
  assert.doesNotMatch(session, /download-artifact|needs\.agent/);
  assert.match(job('apply'), /\n {4}needs: \[policy, agent, check, session\]\n/);
});

// GitHub refuses a call with an input the called workflow does not declare,
// so passing `session` to a policy.yml without it fails every run, session
// or not. Releases before v0.2.0 predate agentic-job's listening bridge
// (its #588).
test('agentic-job is called at a release whose policy has the session input', () => {
  const [, major, minor] = workflow.match(/agentic-job\/\.github\/workflows\/policy\.yml@[0-9a-f]{40} # v(\d+)\.(\d+)\.\d+\n/).map(Number);
  assert.ok(major > 0 || minor >= 2, `v${major}.${minor} has no session input`);
});

// Only claude and the scripted agent: the others need configuration and
// pools this deployment does not have.
test('agents other than fake and claude are refused', async () => {
  for (const agent of ['opencode', 'vibe', '', 'Claude']) {
    await assert.rejects(target({ AGENT: agent }), /AGENT must be fake or claude/, agent);
  }
});

// No proxy is assumed: claude names every setting it lacks, before a
// runner is spent; the scripted agent needs none of them.
test('claude needs the proxy, its audience and the apply environment; fake needs none', async () => {
  const none = { INFERENCE_URL: '', INFERENCE_AUDIENCE: ' ', APPLY_ENVIRONMENT: '' };
  await assert.rejects(target(none),
    /claude needs the apply-environment input \(or the variable APPLY_ENVIRONMENT\), the inference-url input \(or the variable INFERENCE_URL\), the inference-audience input \(or the variable INFERENCE_AUDIENCE\)/);
  await assert.rejects(target({ INFERENCE_URL: '' }), /^Error: claude needs the inference-url input \(or the variable INFERENCE_URL\)$/);
  const fake = await target({ ...none, AGENT: 'fake' });
  assert.equal(fake['inference-url'], '');
  assert.equal(fake['inference-audience'], '');
  assert.equal(fake.model, '');
});

// The join's client id and tags are the calling repository's variables:
// the jmarrero-forge entry of tailnets.json leaves them empty.
test('the tailnet join takes its client and tags from the caller\'s variables', () => {
  assert.match(job('agent'), /\n {10}oauth-client-id: \$\{\{ vars\.TAILSCALE_OAUTH_CLIENT_ID \}\}\n {10}tags: \$\{\{ vars\.TAILSCALE_TAGS \}\}\n/);
});
