import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {existsSync,mkdtempSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createInterface} from 'node:readline';
import {fileURLToPath} from 'node:url';
import test from 'node:test';
import {removePrivateAppFixture} from './private-app-cleanup.mjs';

assert.equal(process.platform,'win32','actual Windows deletion-reference tests required');
const refusal=error=>['EPERM','EBUSY','EACCES'].includes(error.code);
async function lockedFixture(mode) {
  const root=mkdtempSync(join(tmpdir(),'diyime-app-smoke-file-lock-'));
  const file=join(root,'locked.txt');writeFileSync(file,'synthetic fixture');
  const python=process.env.DIY_CLEANUP_PYTHON ?? 'python';
  const child=spawn(python,[fileURLToPath(new URL('./private-app-file-lock-fixture.py',import.meta.url)),file,mode],
    {windowsHide:true,stdio:'pipe'});
  let stderr='',released=false,resolveHeld,rejectHeld;
  const held=new Promise((resolve,reject)=>{resolveHeld=resolve;rejectHeld=reject;});
  child.stdin.on('error',()=>{});
  child.stderr.on('data',part=>{stderr=(stderr+part).slice(-8192);});
  const exited=new Promise(resolve=>{
    child.once('error',error=>{rejectHeld(error);resolve({code:null,error});});
    child.once('close',code=>{rejectHeld(Error('file-lock fixture ended before hold'));resolve({code});});
  });
  createInterface({input:child.stdout}).on('line',line=>{
    try {
      assert(line.length<4096);const value=JSON.parse(line);
      if(value.event==='held')resolveHeld();
      else {assert.equal(value.event,'released');released=true;}
    } catch(error) {rejectHeld(error);child.stdin.end();}
  });
  let timer;
  const timeout=new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('file-lock fixture setup timed out')),15000);});
  try {await Promise.race([held,timeout]);}
  catch(error) {child.stdin.end();await exited;removePrivateAppFixture(root);throw error;}
  finally {clearTimeout(timer);}
  return {root,file,child,async close(){
    child.stdin.end();const result=await exited;
    assert.equal(result.code,0,stderr);assert(!result.error);assert(released);
  }};
}

test('transient actual Windows deletion refusal clears within bounded retries',async()=>{
  const fixture=await lockedFixture('transient');
  try {
    assert.throws(()=>rmSync(fixture.file),refusal,'prove the actual file reference denies deletion');
    fixture.child.stdin.write('r');
    removePrivateAppFixture(fixture.root);
    assert.equal(existsSync(fixture.root),false);
  } finally {
    await fixture.close();
    if(existsSync(fixture.root))removePrivateAppFixture(fixture.root);
  }
});

test('persistent actual Windows refusal remains an error and is not ignored',async()=>{
  const fixture=await lockedFixture('persistent');
  try {
    assert.throws(()=>rmSync(fixture.file),refusal);
    assert.throws(()=>removePrivateAppFixture(fixture.root),refusal);
    assert.equal(existsSync(fixture.file),true);
  } finally {
    await fixture.close();removePrivateAppFixture(fixture.root);
  }
});

test('wrong prefix and missing private roots remain explicit failures',()=>{
  const unrelated=mkdtempSync(join(tmpdir(),'diyime-unrelated-cleanup-'));
  try {assert.throws(()=>removePrivateAppFixture(unrelated),/unsafe/);assert(existsSync(unrelated));}
  finally {rmSync(unrelated,{recursive:true});}
  const missing=mkdtempSync(join(tmpdir(),'diyime-app-smoke-missing-'));
  removePrivateAppFixture(missing);
  assert.throws(()=>removePrivateAppFixture(missing),{code:'ENOENT'});
});
