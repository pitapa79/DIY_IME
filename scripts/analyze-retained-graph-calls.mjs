// Native keys/ids are Host-owned. Anonymous Core aliases are NOT mapped by
// execution order in parallel or repeated-kind graphs. No Core-node duration or
// semantic/framework overhead is inferred by this checker.
import assert from 'node:assert/strict';
export function analyzeRetainedGraphCalls({keys,status,executedIds,unexecutedIds=[]}) {
  const result={scope:'Native exact run-key/node-id counts; anonymous Core-node correlation unavailable',
    goalOverheadAvailable:false,semanticWorkMs:null,frameworkOverheadMs:null,
    correlationError:null,expected:0,paired:0,inconsistent:0,rows:[],ringDrops:[]};
  const admitted=keys.filter(key=>key?.runId);
  result.expected=admitted.length*executedIds.length;
  if(admitted.some(key=>key.sessionEpoch!==status.sessionEpoch)) {
    result.correlationError='Session scope mismatch';return result;
  }
  const bundles=status.retainedWorker?.callSamples??[];
  result.ringDrops=bundles.map(b=>({nodeIds:b.nodeIds,dropped:b.calls.dropped}));
  for(const key of admitted)for(const nodeId of unexecutedIds) {
    if(bundles.some(bundle=>bundle.calls.samples.some(s=>bundle.nodeIds[s.stage]===nodeId&&
      s.runId===key.runId&&s.planVersion===key.planVersion&&s.deliveryGeneration===key.deliveryGeneration&&
      (s.inputRevision??null)===(key.inputRevision??null)))) {
      result.correlationError='unexecuted Node produced Native evidence';return result;
    }
  }
  const safe=n=>Number.isSafeInteger(n)&&n>=0;
  for(const key of admitted)for(const nodeId of executedIds) {
    const matches=bundles.flatMap(bundle=>bundle.calls.samples.filter(s=>
      bundle.nodeIds[s.stage]===nodeId&&s.runId===key.runId&&s.planVersion===key.planVersion&&
      s.deliveryGeneration===key.deliveryGeneration&&(s.inputRevision??null)===(key.inputRevision??null)));
    const row={key,nodeId,status:'missing',semanticWorkMs:null,frameworkOverheadMs:null};
    if(matches.length===1&&matches[0].outcome==='success'&&matches[0].native) {
      const sample=matches[0],n=sample.native;
      if(!['invocationNs','guestCallNs','guestCallbackNs','resourceWaitNs','resourceReads'].every(k=>safe(n[k]))||
        !safe(sample.parentCallNs)||n.guestCallbackNs>n.guestCallNs||n.guestCallNs>n.invocationNs||
        n.guestCallbackNs>n.resourceWaitNs||n.resourceWaitNs>n.invocationNs||n.invocationNs>sample.parentCallNs) {
        row.status='inconsistent';result.inconsistent++;
      } else {
        Object.assign(row,{status:'paired',native:n,parentCallNs:sample.parentCallNs,
          generation:sample.generation,processId:sample.processId});result.paired++;
      }
    } else row.reason=matches.length>1?'duplicate Native rows':matches.length===0?'Native row absent':'non-success or metadata absent';
    result.rows.push(row);
  }
  return result;
}
export function verifyGraphCallCoverage(analysis,latestRunId) {
  assert.equal(analysis.correlationError,null);
  assert.equal(analysis.inconsistent,0);
  assert(analysis.ringDrops.every(row=>row.dropped===0),'Native ring loss prevents complete coverage');
  const rows=latestRunId?analysis.rows.filter(row=>row.key.runId===latestRunId):analysis.rows;
  assert(rows.length>0&&rows.every(row=>row.status==='paired'),'executed Node coverage is incomplete');
}
