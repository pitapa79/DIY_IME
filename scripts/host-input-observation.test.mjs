import test from 'node:test';
import assert from 'node:assert/strict';
import {measureHostInput} from './host-input-scenarios.mjs';

const steps=[{id:'first',text:'synthetic-a',expected:'a'},{id:'latest',text:'synthetic-b',expected:'b'}];
const defaults={steps,rounds:1,pollMs:1,ended:s=>s.ended===true,verify:()=>{},
  summarize:values=>({count:values.length}),submit:async()=>({runId:'1'})};
const terminal={current:true,result:{reason:'completed'},ended:true};

test('read failure retains admitted key, unknown end and unsubmitted latest denominator',async()=>{
  const rounds=[],failure=Error('result-gone');
  await assert.rejects(measureHostInput({...defaults,observeRound:r=>rounds.push(r),read:async()=>{throw failure;}}),e=>e===failure);
  assert.equal(rounds.length,1);
  const r=rounds[0];assert.equal(r.plannedInputs,2);assert.equal(r.complete,false);assert.equal(r.attempts.length,1);
  assert.equal(r.attempts[0].key.runId,'1');assert.equal(r.attempts[0].admissionStatus,'admitted');
  assert.equal(r.attempts[0].endedMs,null);assert.equal(r.attempts[0].responseMs,null);assert.equal(r.attempts[0].outcome,null);
  assert.equal(r.attempts[0].observationFailure.message,'result-gone');assert.equal(r.failure.message,'result-gone');
  assert.equal(r.latestInput.attempted,false);assert.equal(r.latestInput.outcome,null);
  assert.deepEqual(r.counts,{planned:2,attempted:1,notAttempted:1,admitted:1,admissionRejected:0,unresolvedAdmission:0,
    correct:0,superseded:0,cancelled:0,expired:0,failed:0,unclassified:1,physicalEndObserved:0,physicalEndUnknown:1});
});

test('submission error is an attempted unknown admission, not busy or proven no-process',async()=>{
  let progress;const failure=Error('transport-broken');
  await assert.rejects(measureHostInput({...defaults,steps:steps.slice(0,1),observeRound:r=>{progress=r;},
    submit:async()=>{throw failure;}}),e=>e===failure);
  assert.equal(progress.attempts.length,1);assert.equal(progress.attempts[0].admissionStatus,'error');
  assert.equal(progress.attempts[0].submissionFailure.message,'transport-broken');
  assert.equal(progress.attempts[0].admissionMs,null);assert.equal(progress.attempts[0].endedMs,null);
  assert.equal(progress.latestInput.attempted,true);assert.equal(progress.latestInput.physicalEndMs,null);
  assert.equal(progress.counts.unresolvedAdmission,1);assert.equal(progress.counts.physicalEndObserved,0);
});

test('begin and finish hook failures preserve the partial round without claiming completion',async()=>{
  for(const hook of ['beginRound','finishRound']){
    let progress;const failure=Error(hook);
    await assert.rejects(measureHostInput({...defaults,steps:steps.slice(0,1),read:async()=>terminal,
      observeRound:r=>{progress=r;},[hook]:async()=>{throw failure;}}),e=>e===failure);
    assert.equal(progress.complete,false);assert.equal(progress.failure.message,hook);
    assert.equal(progress.attempts.length,hook==='beginRound'?0:1);
    assert.equal(progress.latestInput.outcome,hook==='beginRound'?null:'correct');
  }
});

test('verification failure keeps its reason and does not enter correct-response statistics',async()=>{
  const rounds=[];const result=await measureHostInput({...defaults,steps:steps.slice(0,1),read:async()=>terminal,
    observeRound:r=>rounds.push(r),verify:()=>{throw Error('wrong-output');}});
  assert.equal(rounds.length,3);
  for(const r of rounds){assert.equal(r.attempts[0].verificationFailure.message,'wrong-output');assert.equal(r.latestInput.outcome,'failed');}
  for(const s of result){assert.equal(s.correct,0);assert.equal(s.failed,1);assert.equal(s.firstCorrectResponse,null);}
});

test('fair polling captures older stale physical end before a delayed latest result, without rereading expired proof',{timeout:5000},async()=>{
  let submitted,staleReads,newestReads,interval;const rounds=[];
  const result=await measureHostInput({...defaults,independentInput:true,intervals:[20,100,null],
    observeRound:r=>{rounds.push(r);submitted=0;staleReads=0;newestReads=0;interval=r.intervalMs;},
    submit:async()=>({runId:String(++submitted)}),
    read:async key=>{
      if(Number(key.runId)<submitted){
        if(++staleReads>1)throw Error('expired-after-stale-end-proof');
        return {current:false,result:null,ended:true};
      }
      if(interval!==null){
        if(submitted===1)return {current:true,result:null,ended:false};
        if(!staleReads){assert(++newestReads<8,'older-observation-starved');return {current:true,result:null,ended:false};}
      }
      return terminal;
    }});
  assert.equal(result.length,3);
  for(const r of rounds){assert.equal(r.complete,true);assert.equal(r.latestInput.outcome,'correct');assert(r.attempts.every(a=>a.endedMs!==null));}
  for(const r of rounds.slice(0,2))assert.equal(typeof r.attempts[0].stalePhysicalEndObservedMs,'number');
});

test('a past current result is not stale proof: unavailable final reread still fails',async()=>{
  let submitted=0,progress;
  await assert.rejects(measureHostInput({...defaults,observeRound:r=>{progress=r;},
    submit:async()=>({runId:String(++submitted)}),read:async key=>{
      if(key.runId==='1'&&submitted===2)throw Error('result-gone');return terminal;
    }}),/result-gone/);
  assert.equal(progress.complete,false);assert.equal(progress.attempts[0].outcome,'correct');
  assert.equal(progress.attempts[0].observationFailure.message,'result-gone');
  assert.equal(progress.latestInput.outcome,'correct');
});

test('busy admission remains in the denominator and has no fabricated physical end',async()=>{
  let issued=0;const rounds=[];
  const result=await measureHostInput({...defaults,observeRound:r=>{rounds.push(r);issued=0;},
    submit:async()=>++issued===1?{admissionRejected:'busy'}:{runId:'2'},read:async()=>terminal});
  for(const s of result){assert.equal(s.attempts,2);assert.equal(s.admitted,1);assert.equal(s.admissionRejected,1);assert.equal(s.physicalEnd.count,1);}
  for(const r of rounds){assert.equal(r.attempts[0].admissionStatus,'rejected');assert.equal(r.attempts[0].key,null);assert.equal(r.latestInput.outcome,'correct');}
});

test('an outstanding result read does not block scheduled input submission',{timeout:5000},async()=>{
  let submitted,interval,release,readInFlight;const rounds=[];
  await measureHostInput({...defaults,independentInput:true,intervals:[20,100,null],
    observeRound:r=>{rounds.push(r);submitted=0;interval=r.intervalMs;release=null;readInFlight=false;},
    submit:async()=>{const runId=String(++submitted);if(submitted===2&&interval!==null){assert(readInFlight);release();}return {runId};},
    read:async key=>{
      if(submitted===1&&interval!==null){readInFlight=true;await new Promise(resolve=>{release=resolve;});readInFlight=false;}
      return Number(key.runId)<submitted?{current:false,result:null,ended:true}:terminal;
    }});
  assert(rounds.every(r=>r.complete&&r.latestInput.outcome==='correct'));
});

test('failed submission settles the outstanding read and preserves both failures',{timeout:5000},async()=>{
  let progress,release,submitted=0;
  const failure=Error('second-submit-failed');
  await assert.rejects(measureHostInput({...defaults,independentInput:true,intervals:[20,100,null],
    observeRound:r=>{progress=r;},
    submit:async()=>{if(++submitted===2){release();throw failure;}return {runId:'1'};},
    read:async()=>{await new Promise(resolve=>{release=resolve;});throw Error('first-read-failed');}}),e=>e===failure);
  assert.equal(progress.failure.message,'second-submit-failed');
  assert.equal(progress.attempts[0].observationFailure.message,'first-read-failed');
  assert.equal(progress.attempts[1].submissionFailure.message,'second-submit-failed');
  assert.equal(progress.counts.unresolvedAdmission,1);assert.equal(progress.counts.physicalEndUnknown,1);
});

test('a stuck outstanding read cannot block failure finalization or mutate returned evidence',{timeout:4000},async()=>{
  let progress,release,submitted=0;const failure=Error('submission-failed');
  await assert.rejects(measureHostInput({...defaults,independentInput:true,intervals:[20,100,null],
    observeRound:r=>{progress=r;},
    submit:async()=>{if(++submitted===2)throw failure;return {runId:'1'};},
    read:async()=>{await new Promise(resolve=>{release=resolve;});return terminal;}}),e=>e===failure);
  assert.equal(progress.outstandingReadUnsettled,true);assert.equal(progress.failure.message,'submission-failed');
  assert.equal(progress.attempts[0].endedMs,null);assert.equal(progress.counts.physicalEndUnknown,1);
  const frozen=JSON.stringify(progress);release();await new Promise(resolve=>setImmediate(resolve));
  assert.equal(JSON.stringify(progress),frozen);
});
