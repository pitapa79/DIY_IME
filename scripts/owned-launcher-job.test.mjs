import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import test from 'node:test';
import {setTimeout as delay} from 'node:timers/promises';
import {OwnedLauncherJob} from './owned-launcher-job.mjs';

test('actual Windows wrapper protocol proves zero before outer close',
  {skip:process.platform!=='win32',timeout:15000},async()=>{
    const python=spawnSync(process.env.DIY_WINDOWS_TEST_PYTHON || 'python',
      ['-c','import sys;print(sys.executable)'],{encoding:'utf8',windowsHide:true});
    assert.equal(python.status,0,python.stderr);
    const owner=new OwnedLauncherJob([python.stdout.trim(),'-u','-c',
      'import json,time;print(json.dumps({"ready":True}),flush=True);time.sleep(30)']);
    try {
      const start=await owner.started;
      assert.equal(start.startup.ready,true);
      const members=await owner.call('members');
      assert(members.members.some(member=>member.pid===start.pid));
      await assert.rejects(owner.call('assertEmpty'),/left owned Job members/);
      assert.equal((await owner.call('crash')).originalHandleSignalled,true);
      const deadline=Date.now()+5000;
      while((await owner.call('members')).active){assert(Date.now()<deadline);await delay(10);}
      assert.equal((await owner.call('assertEmpty')).emptyBeforeJobClose,true);
    } finally {await owner.close();}
  });
