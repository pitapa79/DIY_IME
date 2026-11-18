import assert from 'node:assert/strict';
import test from 'node:test';
import {preparationPhaseReader} from './package-preparation-phases.mjs';

test('streams only complete bounded phase records, across split chunks',()=>{
  const records=[];
  const read=preparationPhaseReader(row=>records.push(row));
  const valid='package-preparation id=1 phase=worker-launch-and-settle event=start us=0';
  read(Buffer.from('private content\n'+'x'.repeat(180)+valid+'\r\n'+valid.slice(0,41)));
  read(Buffer.from(valid.slice(41)+'\r\n'+valid.replace('id=1','id=128')+'\n'+valid.replace('us=0','us=1')+'\n'));
  assert.deepEqual(records,[{id:1,phase:'worker-launch-and-settle',event:'start',elapsedUs:0}]);
});

test('does not forward more than the fixed per-process budget',()=>{
  let count=0;
  const read=preparationPhaseReader(()=>count++);
  read(Buffer.from('package-preparation id=1 phase=worker-cleanup event=complete us=42\n'.repeat(300)));
  assert.equal(count,256);
});

test('accepts only named snapshot and ACL phases',()=>{
  const records=[];
  const read=preparationPhaseReader(row=>records.push(row));
  const phases=['worker-snapshot-directory','worker-snapshot-image-write','worker-snapshot-dll-write',
    'worker-snapshot-module-write','worker-snapshot-locks','worker-runtime-acl-lease'];
  for(const [id,phase] of [...phases,'worker-snapshot-C:\\private'].entries()) {
    read(Buffer.from(`package-preparation id=${id} phase=${phase} event=complete us=42\n`));
  }
  assert.deepEqual(records,phases.map((phase,id)=>({id,phase,event:'complete',elapsedUs:42})));
});
