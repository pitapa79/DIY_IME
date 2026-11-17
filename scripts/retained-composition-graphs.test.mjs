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
