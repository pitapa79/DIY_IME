import test from 'node:test';
import assert from 'node:assert/strict';
import {analyzeLinearCallTiming} from './analyze-retained-call-timing.mjs';

function fixture() {
  const key={sessionEpoch:'epoch',runId:'7',planVersion:'1',deliveryGeneration:'2',inputRevision:3};
  return {keys:[key],ids:['a'],kinds:['demo.identity'],events:[
    {event:'run-accepted',run:99,atUs:'0'},
    {event:'node-started',run:99,node:'1:2',nodeKind:'demo.identity',atUs:'1'},
    {event:'node-resolved',run:99,node:'1:2',atUs:'4001'},
  ],status:{sessionEpoch:'epoch',retainedWorker:{callSamples:[{nodeIds:['a'],calls:{dropped:0,samples:[{
    ...key,stage:0,generation:1,processId:10,parentCallNs:3_000_000,native:{
      invocationNs:2_800_000,guestCallNs:2_000_000,guestCallbackNs:500_000,
      resourceWaitNs:500_000,resourceReads:1,
    },
  }]}}]}}};
}
test('derive each Node overhead from its own call, keeping callback wait in framework time',()=>{
  const result=analyzeLinearCallTiming(fixture());
  assert.equal(result.correlationError,null);
  assert.equal(result.paired,1);
  assert.equal(result.rows[0].guestExclusiveMs,1.5);
  assert.equal(result.rows[0].nodeOverheadMs,2.5);
  assert.equal(result.rows[0].parentFrameworkMs,1.5);
});
test('missing, duplicate or different delivery metadata never becomes a zero-duration sample',()=>{
  for(const kind of ['absent','duplicate','wrong-delivery']) {
    const value=fixture(),samples=value.status.retainedWorker.callSamples[0].calls.samples;
    if(kind==='absent')samples[0].native=null;
    if(kind==='duplicate')samples.push(structuredClone(samples[0]));
    if(kind==='wrong-delivery')samples[0].deliveryGeneration='3';
    const result=analyzeLinearCallTiming(value);
    assert.equal(result.paired,0);assert.equal(result.missing,1);
    assert.equal(result.rows[0].nodeOverheadMs,undefined);
  }
});
test('invalid duration relationships and unsupported correlation are reported explicitly',()=>{
  const value=fixture();value.status.retainedWorker.callSamples[0].calls.samples[0].native.guestCallbackNs=2_100_000;
  const result=analyzeLinearCallTiming(value);
  assert.equal(result.inconsistent,1);assert.equal(result.paired,0);
  const other=fixture();other.keys.push({...other.keys[0],runId:'8'});
  assert(analyzeLinearCallTiming(other).correlationError);
  const wrong=fixture();wrong.events[1].nodeKind='another-node';
  assert(analyzeLinearCallTiming(wrong).correlationError);
});
