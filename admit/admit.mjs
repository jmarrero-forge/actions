import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

// Whether the calling repository's owner is one of ours, by its numeric id:
// a login can be renamed and then registered by someone else, an id cannot.
// The names in owners.json are for the reader only.
export function admitted(ownerId) {
  const owners = JSON.parse(readFileSync(new URL('./owners.json', import.meta.url), 'utf8'));
  return /^[1-9][0-9]*$/.test(ownerId ?? '') && Object.values(owners).some(id => String(id) === ownerId);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (!admitted(process.env.OWNER_ID)) {
    console.log(`::error::${process.env.OWNER} (id ${process.env.OWNER_ID}) is not an owner admit/owners.json lists`);
    process.exit(1);
  }
  console.log(`${process.env.OWNER} (id ${process.env.OWNER_ID}) is admitted`);
}
