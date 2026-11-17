import test from 'node:test';
import assert from 'node:assert/strict';
import {observeWarmInput,warmInputDenominators} from './host-warm-input-observation.mjs';
const key={runId:'1'};
function setup(overrides={}) {let now=0;return {sample:0,attempts:[],submit:async()=>key,read:async()=>({state:'terminal',current:true,physical:{jobs:0}}),
  until:async check=>check(),ended:s=>s.state==='terminal',verify:()=>{},expected:'synthetic',clock:()=>++now,...overrides};}
test('warm failure retains known admission/key and unknown physical end',async()=>{
  const a=setup({read:async()=>{throw Error('result-gone');}});
  await assert.rejects(observeWarmInput(a),/result-gone/);
  assert.deepEqual(a.attempts[0].key,key);assert.equal(a.attempts[0].physicalEndMs,null);
  assert.deepEqual(warmInputDenominators(a.attempts,4),{planned:4,attempted:1,unsubmitted:3,admitted:1,admissionRejected:0,unknownAdmission:0,correct:0,failedOrUnknown:1,physicalEndObserved:0});
});
test('submission failure is unknown admission; busy has no invented physical end',async()=>{
  const a=setup({submit:async()=>{throw Error('pipe');}});
  await assert.rejects(observeWarmInput(a),/pipe/);assert.equal(a.attempts[0].admitted,null);
  const b=setup({submit:async()=>({admissionRejected:'busy'})});
  await assert.rejects(observeWarmInput(b),/busy/);assert.equal(b.attempts[0].admitted,false);assert.equal(b.attempts[0].physicalEndMs,null);
});
test('verification failure retains true ending without entering correct distributions',async()=>{
  const a=setup({verify:()=>{throw Error('wrong-output');}});
  await assert.rejects(observeWarmInput(a),/wrong-output/);
  assert.equal(a.attempts[0].outcome,'verification-error');assert(a.attempts[0].physicalEndMs>0);
  assert.equal(warmInputDenominators(a.attempts,2).correct,0);
  const b=setup();const result=await observeWarmInput(b);assert(result.resultMs>0);assert.equal(b.attempts[0].outcome,'correct');
});

test('client verification cost stays outside the existing response measurement endpoint',async()=>{
  let now=0;
  const a=setup({clock:()=>now,submit:async()=>{now=2;return key;},read:async()=>{now=5;return {state:'terminal',current:true,physical:{jobs:0}};},verify:()=>{now+=1000;}});
  const result=await observeWarmInput(a);
  assert.equal(result.resultMs,5);assert.equal(a.attempts[0].physicalEndMs,5);
});
