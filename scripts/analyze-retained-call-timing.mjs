// Controlled linear benchmark only. Core trace aliases intentionally hide Graph
// IDs: correlate admission order and strict dependency order, then verify kinds.
// Do not apply this mapping to parallel/branching/retried arbitrary graphs.
export function analyzeLinearCallTiming({keys, events, status, ids, kinds}) {
  const admitted=keys.filter(key=>key?.runId);
  const accepted=events.filter(event=>event.event==='run-accepted');
  const result={scope:'same-call linear benchmark; wall time, not CPU',expected:admitted.length*ids.length,
    correlationError:null,executed:0,notExecuted:0,paired:0,missing:0,inconsistent:0,rows:[],ringDrops:[]};
  if(accepted.length!==admitted.length || admitted.some(key=>key.sessionEpoch!==status.sessionEpoch)) {
    result.correlationError='trace admission count or Session scope mismatch';
    return result;
  }
  const bundles=status.retainedWorker?.callSamples ?? [];
  result.ringDrops=bundles.map(bundle=>({nodeIds:bundle.nodeIds,dropped:bundle.calls.dropped}));
  const safe=value=>Number.isSafeInteger(value) && value>=0;
  for(let run=0;run<admitted.length;run++) {
    const key=admitted[run],alias=accepted[run].run;
    const runEvents=events.filter(event=>event.run===alias);
    const started=runEvents.filter(event=>event.event==='node-started' && event.nodeKind!=='input.text');
    if(started.length>ids.length || started.some((event,index)=>event.nodeKind!==kinds[index])) {
      result.correlationError='trace is not the expected strict linear execution';
      return result;
    }
    for(let index=0;index<ids.length;index++) {
      const nodeId=ids[index],start=started[index];
      const row={key,nodeId,kind:kinds[index],status:'not-executed'};
      if(!start) {result.notExecuted++;result.rows.push(row);continue;}
      result.executed++;
      const finish=runEvents.find(event=>event.node===start.node &&
        ['node-resolved','node-no-output','node-failed','node-discarded'].includes(event.event));
      const matches=bundles.flatMap(bundle=>bundle.calls.samples.filter(sample=>
        bundle.nodeIds[sample.stage]===nodeId && sample.runId===key.runId &&
        sample.planVersion===key.planVersion && sample.deliveryGeneration===key.deliveryGeneration &&
        (sample.inputRevision ?? null)===(key.inputRevision ?? null)));
      row.traceNode=start.node;
      if(matches.length!==1 || !finish || !matches[0].native) {
        row.status='missing';row.reason=matches.length>1?'duplicate Native rows':
          !finish?'unfinished Node trace':!matches.length?'Native row absent':'Native metadata absent';
        result.missing++;result.rows.push(row);continue;
      }
      const sample=matches[0],native=sample.native;
      const fields=['invocationNs','guestCallNs','guestCallbackNs','resourceWaitNs','resourceReads'];
      const nodeNs=Number(BigInt(finish.atUs)-BigInt(start.atUs))*1000;
      if(!fields.every(field=>safe(native[field])) || !safe(sample.parentCallNs) || !safe(nodeNs) ||
        native.guestCallbackNs>native.guestCallNs || native.guestCallNs>native.invocationNs ||
        native.guestCallbackNs>native.resourceWaitNs || native.resourceWaitNs>native.invocationNs ||
        native.invocationNs>sample.parentCallNs || native.guestCallNs-native.guestCallbackNs>nodeNs) {
        row.status='inconsistent';row.native=native;row.parentCallNs=sample.parentCallNs;
        row.nodeNs=nodeNs;result.inconsistent++;result.rows.push(row);continue;
      }
      const guestExclusiveNs=native.guestCallNs-native.guestCallbackNs;
      Object.assign(row,{status:'paired',generation:sample.generation,processId:sample.processId,
        nodeMs:nodeNs/1e6,guestExclusiveMs:guestExclusiveNs/1e6,
        guestInclusiveMs:native.guestCallNs/1e6,guestCallbackMs:native.guestCallbackNs/1e6,
        resourceWaitMs:native.resourceWaitNs/1e6,resourceReads:native.resourceReads,
        nativeInvocationMs:native.invocationNs/1e6,parentCallMs:sample.parentCallNs/1e6,
        nodeOverheadMs:(nodeNs-guestExclusiveNs)/1e6,
        nativeFrameworkMs:(native.invocationNs-guestExclusiveNs)/1e6,
        parentFrameworkMs:(sample.parentCallNs-guestExclusiveNs)/1e6});
      result.paired++;result.rows.push(row);
    }
  }
  return result;
}
