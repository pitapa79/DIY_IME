// Test fixtures only. Call after the existing process/Job ending assertions.
import {lstatSync,rmSync} from 'node:fs';
import {basename,dirname,join,resolve} from 'node:path';
import {tmpdir} from 'node:os';

export function removePrivateAppFixture(root) {
  const path=resolve(root),parent=resolve(tmpdir()),prefix='diyime-app-smoke-';
  if(dirname(path)!==parent || !basename(path).startsWith(prefix)
      || basename(path).length<=prefix.length || !path.startsWith(join(parent,prefix)))
    throw Error('unsafe private app fixture cleanup');
  if(lstatSync(path).isSymbolicLink())throw Error('private app fixture root is a link');
  // Node retries a transient EPERM/EBUSY/etc. for each failing operation.
  // Eight linear50ms backoffs total at most1800ms of retry sleep per operation.
  // Keep force=false: persistent refusal and missing roots remain failures.
  rmSync(path,{recursive:true,maxRetries:8,retryDelay:50});
}
