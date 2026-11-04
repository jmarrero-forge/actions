import { appendFileSync, readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function settings(owner, overrides = {}) {
  const tailnets = JSON.parse(readFileSync(new URL('./tailnets.json', import.meta.url)));
  // An organization without an entry, another deployment's, names its own
  // login: a client ID and tags, both, which our wrappers take from the
  // calling repository's variables. That admits no one new: Tailscale's
  // federated identity decides which repositories may log in as a client
  // ID, and an entry's settings could already be overridden all the same.
  const known = Object.hasOwn(tailnets, owner);
  if (!known && !(overrides['oauth-client-id'] && overrides.tags)) {
    throw new Error(`No tailnet settings for GitHub organization ${owner}; add an entry to tailscale/tailnets.json, or give both oauth-client-id and tags.`);
  }
  const result = known ? { ...tailnets[owner] } : { 'oauth-client-id': '', tags: '', proxy: '' };
  for (const key of ['oauth-client-id', 'tags', 'proxy']) {
    if (overrides[key]) result[key] = overrides[key];
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
