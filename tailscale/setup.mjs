import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function settings(owner, overrides = {}) {
  const tailnets = JSON.parse(readFileSync(new URL('./tailnets.json', import.meta.url)));
  if (!Object.hasOwn(tailnets, owner)) {
    throw new Error(`No tailnet settings for GitHub organization ${owner}; add an entry to tailscale/tailnets.json.`);
  }
  const result = { ...tailnets[owner] };
  for (const key of ['oauth-client-id', 'tags', 'proxy']) {
    if (overrides[key]) result[key] = overrides[key];
  }
  // An entry may leave these empty for the caller to pass as inputs, from
  // values only the tailnet's admin has; joining without them would fail
  // later with a less helpful error from Tailscale.
  for (const key of ['oauth-client-id', 'tags']) {
    if (!result[key]) {
      throw new Error(`No ${key} for GitHub organization ${owner}: set it in tailscale/tailnets.json or pass the ${key} input.`);
    }
  }
  result.audience = overrides.audience || `api.tailscale.com/${result['oauth-client-id']}`;
  for (const value of Object.values(result)) {
    if (/[\r\n]/.test(value)) throw new Error('Tailnet settings must be single-line values');
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const overrides = Object.fromEntries(['oauth-client-id', 'audience', 'tags', 'proxy']
    .map(key => [key, process.env[key.toUpperCase().replaceAll('-', '_')]]));
  const result = settings(process.env.REPOSITORY_OWNER, overrides);
  appendFileSync(process.env.GITHUB_OUTPUT,
    Object.entries(result).map(([key, value]) => `${key}=${value}\n`).join(''));
}
