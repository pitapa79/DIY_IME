import test from 'node:test';
import assert from 'node:assert/strict';
import {analyzeRetainedLifecycle} from './analyze-retained-lifecycle.mjs';
const event=(phase,atNs,attempt='1')=>({phase,atNs,attempt,processId:17,runId:null});
function status(phases,epoch='epoch') {
  return {sessionEpoch:epoch,state:{activePlanVersion:'1'},retainedWorker:{callSamples:[{
    nodeIds:['node'],calls:{phaseDropped:0,phases},
  }]}};
}
test('deduplicate snapshots and use same-cohort differences without hiding missing spans',()=>{
  const phases=[event('profile-create-started','1000000'),event('profile-create-finished','4000000')];
  const result=analyzeRetainedLifecycle([status(phases),status(phases)]);
  assert.equal(result.owners[0].observedEvents,2);
  assert.equal(result.owners[0].attempts[0].spans.profileCreate.ms,3);
  assert.equal(result.owners[0].attempts[0].spans.aclRelease.state,'not-observed');
  assert.equal(result.owners[0].attempts[0].spans.aclRelease.ms,undefined);
});
test('separate contexts and report already-ended or invalid order without negative/clamped latency',()=>{
  const phases=[event('original-process-io-ended','1000'),event('retirement-requested','2000')];
  const result=analyzeRetainedLifecycle([status(phases),status(phases,'next')]);
  assert.equal(result.owners.length,2);
  const span=result.owners[0].attempts[0].spans.retirementToOriginalExit;
  assert.equal(span.state,'exit-before-retirement-request');assert.equal(span.ms,undefined);
});
test('measure launch stages with observed endpoints and preserve a missing resume result',()=>{
  const phases=[event('native-launch-started','1000000'),
    event('native-create-process-started','2000000'),event('native-create-process-finished','7000000'),
    event('native-job-token-check-started','8000000'),event('native-job-token-confirmed','10000000'),
    event('native-resume-thread-started','15000000'),event('readiness-observed','35000000')];
  const spans=analyzeRetainedLifecycle([status(phases)]).owners[0].attempts[0].spans;
  for(const [name,ms] of Object.entries({launchSetup:1,createSuspendedProcess:5,createdToVerification:1,confirmJobAndToken:2,
    verifiedToResume:5,launchToReadiness:34})) assert.equal(spans[name].ms,ms);
  for(const name of ['resumeThread','resumedToReadiness']) assert.equal(spans[name].state,'not-observed');
  const complete=analyzeRetainedLifecycle([status([...phases,event('native-resume-thread-finished','16000000')])]);
  assert.equal(complete.owners[0].attempts[0].spans.resumedToReadiness.ms,19);
});
