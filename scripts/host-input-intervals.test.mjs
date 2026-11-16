import test from 'node:test';
import assert from 'node:assert/strict';
import {measureHostInput} from './host-input-scenarios.mjs';
test('cold-first pacing changes order while preserving all three input cadences',async()=>{
  const seen=[];
  const result=await measureHostInput({steps:[{id:'only',text:'x',expected:'x'}],rounds:1,pollMs:1,
    intervals:[20,100,null],submit:async()=>({id:1}),read:async()=>({current:true,result:{}}),
    ended:()=>true,verify:()=>{},summarize:values=>({count:values.length}),
    finishRound:async interval=>seen.push(interval)});
  assert.deepEqual(seen,[20,100,null]);
  assert.deepEqual(result.map(item=>item.intervalMs),seen);
});
test('a reordered experiment cannot silently omit, duplicate or replace an original cadence',async()=>{
  for(const intervals of [[],[20],[20,20,null],[20,100,10]]) {
    let submitted=false;
    await assert.rejects(()=>measureHostInput({intervals,rounds:1,submit:async()=>{submitted=true;}}),
      /all-three-input-intervals-required/);
    assert.equal(submitted,false);
  }
});
