import test from 'node:test';
import assert from 'node:assert/strict';
import {retainedComposition,fixtureDefinition,fixtureKinds} from './retained-composition-graphs.mjs';
import {analyzeRetainedGraphCalls,verifyGraphCallCoverage} from './analyze-retained-graph-calls.mjs';

test('matrix graphs preserve Text inputs, true parallel dependencies, reconvergence and terminal Any outputs',()=>{
  for(const count of [1,2,4,8,16,20])for(const layout of ['serial','parallel','unused',...(count>=4?['join']:[])]) {
    const spec=retainedComposition(count,layout),nodes=new Map(spec.graph.nodes.map(n=>[n.id,n]));
    const outputRoot=node=>node.kind==='demo.identity'?'any':'text';
    for(const edge of spec.graph.edges) {
      assert(nodes.has(edge.from.node)&&nodes.has(edge.to.node));
      assert(edge.to.node!==edge.from.node);
      if(nodes.get(edge.to.node).kind!=='demo.identity')assert.equal(outputRoot(nodes.get(edge.from.node)),'text');
    }
    const upstream=new Set();
    function visit(id){if(upstream.has(id))return;upstream.add(id);for(const e of spec.graph.edges)if(e.to.node===id)visit(e.from.node);}
    spec.targets.forEach(t=>visit(t.node));
    assert.deepEqual(spec.executedIds.filter(id=>!upstream.has(id)),[]);
    assert(spec.unexecutedIds.every(id=>!upstream.has(id)));
    if(layout==='parallel')assert(spec.graph.edges.every(e=>e.from.node==='input'));
    if(layout==='join')assert.equal(spec.graph.edges.filter(e=>e.to.node===spec.ids.at(-2)).length,2);
    if(layout!=='join')assert.equal(spec.expected('🙂'),'🙂');
    assert.equal(spec.ids.length,count);
  }
  assert.throws(()=>retainedComposition(2,'join'));
  assert.equal(fixtureDefinition(fixtureKinds[3]).inputs.length,2);
});
const key={sessionEpoch:'s',runId:'7',planVersion:'1',deliveryGeneration:'2',inputRevision:'9'};
function evidence(){return {sessionEpoch:'s',retainedWorker:{callSamples:['a','b'].map(id=>({nodeIds:[id],calls:{dropped:0,samples:[{
  ...key,stage:0,outcome:'success',generation:1,processId:id==='a'?11:12,parentCallNs:20,
  native:{invocationNs:15,guestCallNs:10,guestCallbackNs:2,resourceWaitNs:3,resourceReads:1},
}]}}))}};}
test('parallel Native coverage uses exact keys and ids even if owner order changes, without inventing Core aliases',()=>{
  const status=evidence();status.retainedWorker.callSamples.reverse();
  const a=analyzeRetainedGraphCalls({keys:[key],status,executedIds:['a','b']});
  verifyGraphCallCoverage(a);assert.equal(a.paired,2);assert.equal(a.frameworkOverheadMs,null);
  assert(a.rows.every(row=>!Object.hasOwn(row,'traceNode')));
});
test('wrong generation, duplicate rows, missing nodes and ring loss remain failures',()=>{
  for(const mutate of [s=>{s.retainedWorker.callSamples[0].calls.samples[0].deliveryGeneration='3';},
    s=>{s.retainedWorker.callSamples[0].calls.samples.push({...s.retainedWorker.callSamples[0].calls.samples[0]});},
    s=>{s.retainedWorker.callSamples.pop();},s=>{s.retainedWorker.callSamples[0].calls.dropped=1;}]) {
    const status=evidence();mutate(status);
    assert.throws(()=>verifyGraphCallCoverage(analyzeRetainedGraphCalls({keys:[key],status,executedIds:['a','b']})));
  }
});

function finiteHistory(count=31) {
  return Array.from({length:count},(_,index)=>{
    const n=index+1,status=evidence();
    for(const bundle of status.retainedWorker.callSamples) {
      const original=bundle.calls.samples[0];
      bundle.calls={capacity:16,dropped:Math.max(0,n-16),samples:
        Array.from({length:Math.min(n,16)},(_,offset)=>({...structuredClone(original),
          runId:String(Math.max(1,n-15)+offset)}))};
    }
    if(index%2)status.retainedWorker.callSamples.reverse();
    return status;
  });
}
test('31 observed calls survive a finite16 ring with every evicted row archived and no invented aliases',()=>{
  const snapshots=finiteHistory(),history=[];
  for(let i=0;i<snapshots.length;i++) {
    const analysis=analyzeRetainedGraphCalls({keys:[{...key,runId:String(i+1)}],
      status:snapshots[i],executedIds:['a','b'],history});
    verifyGraphCallCoverage(analysis,String(i+1));
    assert.equal(analysis.paired,2);
    assert(analysis.ringHistoryEvidence.owners.every(o=>o.observedRows===i+1));
    if(i===30)assert(analysis.ringHistoryEvidence.owners.every(o=>o.evictionsPreviouslyObserved===15));
    history.push(snapshots[i]);
  }
});
test('batched observation proves all63 rows across seven-input rounds before finite-ring eviction',()=>{
  const all=finiteHistory(63),history=[];
  for(let end=7;end<=63;end+=7) {
    const analysis=analyzeRetainedGraphCalls({keys:Array.from({length:7},(_,i)=>({...key,runId:String(end-6+i)})),
      status:all[end-1],executedIds:['a','b'],history});
    verifyGraphCallCoverage(analysis,String(end));
    assert.equal(analysis.paired,14);
    assert(analysis.ringHistoryEvidence.owners.every(o=>o.observedRows===end));
    history.push(all[end-1]);
  }
});
test('archive cannot forgive unobserved eviction, changed rows, reset counters, or foreign Sessions',()=>{
  const all=finiteHistory(),latest=all.at(-1),keys=[{...key,runId:'31'}];
  for(const [history,status] of [
    [[],latest],[[all[0]],latest],
    [all.slice(0,-1),(()=>{const s=structuredClone(latest);s.retainedWorker.callSamples[0].calls.samples[0].native.invocationNs++;return s;})()],
    [all.slice(0,-1),(()=>{const s=structuredClone(latest);s.retainedWorker.callSamples[0].calls.dropped=0;return s;})()],
    [[{...all[0],sessionEpoch:'other'}],latest],
  ]) {
    const analysis=analyzeRetainedGraphCalls({keys,status,executedIds:['a','b'],history});
    assert.equal(analysis.ringHistoryEvidence.complete,false);
    assert.throws(()=>verifyGraphCallCoverage(analysis,'31'));
  }
});
test('complete archive still cannot replace missing latest Node evidence',()=>{
  const all=finiteHistory();
  const status=structuredClone(all.at(-1));
  status.retainedWorker.callSamples[0].calls.samples.at(-1).runId='32';
  const analysis=analyzeRetainedGraphCalls({keys:[{...key,runId:'31'}],status,
    executedIds:['a','b'],history:all.slice(0,-1)});
  assert.equal(analysis.ringHistoryEvidence.complete,true);
  assert.equal(analysis.paired,1);
  assert.throws(()=>verifyGraphCallCoverage(analysis,'31'));
});

test('reconvergence verifies distinct branch witnesses and rejects swapped or fabricated outputs',()=>{
  const spec=retainedComposition(4,'join'),wanted=spec.expected('🙂');
  const right=JSON.stringify({apiVersion:1,node:{kind:'fixture.wasm.chain-alt'},
    operation:{config:{mode:'echo'},inputs:{in:{packet:{root:{text:'🙂'}}}}}});
  spec.verify('🙂|'+right,wanted);
  for(const bad of ['🙂|🙂',right+'|🙂','🙂|'+right.replace('chain-alt','chain')])assert.throws(()=>spec.verify(bad,wanted));
});

test('unrequested nodes cannot silently execute, and the predeclared matrix preserves all required dimensions',async()=>{
  const status=evidence();
  const a=analyzeRetainedGraphCalls({keys:[key],status,executedIds:['a'],unexecutedIds:['b']});
  assert.throws(()=>verifyGraphCallCoverage(a));
  const {retainedCompositionCases}=await import('./retained-composition-cases.mjs');
  const cases=retainedCompositionCases();assert.equal(cases.length,25);
  assert.deepEqual([...new Set(cases.filter(c=>c.mode==='separate'&&c.layout==='serial').map(c=>c.count))].sort((a,b)=>a-b),[1,2,4,8,16,20]);
  assert(cases.some(c=>c.cadence==='cold-paced')&&cases.some(c=>c.cadence==='paced'));
  assert(cases.some(c=>c.mode==='separate'&&!c.timing));
});
