import test from 'node:test';
import assert from 'node:assert/strict';
import {PrivateReplyEvidence} from './private-sample-delivery.mjs';

const status={retainedWorker:{callSamples:[{nodeIds:['a'],calls:{dropped:2,samples:[{stage:0,runId:'7'}]}}]}};
test('records actual UTF-8 JSON line bytes separately by operation and no contents',()=>{
  const evidence=new PrivateReplyEvidence();
  const reply={ok:true,data:{snapshot:{text:'秘密🙂'},state:{retainedWorker:{physicalJobs:1}}}};
  const line=JSON.stringify(reply);
  evidence.record('getRun',line,reply);
  assert.deepEqual(evidence.byMethod.getRun,{replies:1,jsonBytes:Buffer.byteLength(line),maxJsonBytes:Buffer.byteLength(line),repliesWithSamples:0});
  evidence.assertExclusive();
  assert(!JSON.stringify(evidence).includes('秘密'));
});
test('detects private buffers embedded in ordinary replies even when empty',()=>{
  for(const method of ['run','getRun','cancelRun','getStart','invalidateInput','definitions','activate']) {
    const evidence=new PrivateReplyEvidence();
    const data=method==='invalidateInput'||method==='activate'?{retainedWorker:{callSamples:[]}}:{state:status};
    const reply={ok:true,data};
    evidence.record(method,JSON.stringify(reply),reply);
    assert.throws(()=>evidence.assertExclusive(),/must not materialize/);
  }
});
test('explicit status preserves samples and drops across repeated non-draining retrieval',()=>{
  const evidence=new PrivateReplyEvidence();
  const reply={ok:true,data:status};
  evidence.record('status',JSON.stringify(reply),reply);
  evidence.verifyRepeatedStatus(status,structuredClone(status),['a']);
  evidence.assertExclusive();
  assert.equal(evidence.byMethod.status.repliesWithSamples,1);
  assert.equal(evidence.repeatedSnapshotsChecked,1);
});
test('missing evidence, changed samples and hidden ring drops fail',()=>{
  const evidence=new PrivateReplyEvidence();
  for(const next of [{retainedWorker:{}},{retainedWorker:{callSamples:[]}},
    {retainedWorker:{callSamples:[{nodeIds:['a'],calls:{dropped:0,samples:[{stage:0,runId:'7'}]}}]}}]) {
    assert.throws(()=>evidence.verifyRepeatedStatus(status,next,['a']));
  }
  assert.throws(()=>evidence.verifyRepeatedStatus(status,status,['unexecuted']));
});
test('OFF/default status never fabricates samples',()=>{
  const evidence=new PrivateReplyEvidence(),empty={retainedWorker:{physicalJobs:0}};
  evidence.verifyRepeatedStatus(empty,structuredClone(empty));
  const reply={ok:true,data:empty};
  evidence.record('status',JSON.stringify(reply),reply);
  assert.equal(evidence.byMethod.status.repliesWithSamples,0);
});
test('ongoing owner cleanup can append phases but cannot hide phase loss or drain evidence',()=>{
  const evidence=new PrivateReplyEvidence(),before=structuredClone(status);
  Object.assign(before.retainedWorker.callSamples[0].calls,{phases:[{phase:'ready'},{phase:'stop'}],phaseDropped:0});
  const appended=structuredClone(before);
  appended.retainedWorker.callSamples[0].calls.phases.push({phase:'original-ended'});
  evidence.verifyRepeatedStatus(before,appended,['a']);
  const evicted=structuredClone(appended);
  evicted.retainedWorker.callSamples[0].calls.phases.shift();
  assert.throws(()=>evidence.verifyRepeatedStatus(before,evicted,['a']),/must not drain phase/);
  evicted.retainedWorker.callSamples[0].calls.phaseDropped=1;
  evidence.verifyRepeatedStatus(before,evicted,['a']);
  evicted.retainedWorker.callSamples[0].calls.phaseDropped=-1;
  assert.throws(()=>evidence.verifyRepeatedStatus(before,evicted,['a']),/loss must remain accounted/);
});
