// All differences below use the same Host cohort clock. These are observations,
// not OS exit receipts or permission/quota decisions. No missing interval is 0.
const pairs={
  startupGate:['attempt-started','startup-permit-acquired'],
  preparationGate:['preparation-permit-waiting','preparation-permit-acquired'],
  codePreparation:['code-preparation-started','code-preparation-finished'],
  profileCreate:['profile-create-started','profile-create-finished'],
  codeAccess:['code-access-started','code-access-finished'],
  jobCreate:['job-create-started','job-create-finished'],
  launchToReadiness:['native-launch-started','readiness-observed'],
  retirementToOriginalExit:['retirement-requested','original-process-io-ended'],
  aclRelease:['acl-release-started','acl-release-finished'],
  profileDelete:['profile-delete-started','profile-delete-finished'],
  retirementToOwnerJoin:['retirement-requested','owner-joined'],
};
function interval(events,begin,end) {
  const starts=events.filter(event=>event.phase===begin),ends=events.filter(event=>event.phase===end);
  if(starts.length!==1 || ends.length!==1 || starts[0].atNs==null || ends[0].atNs==null)
    return {state:'not-observed',beginEvents:starts.length,endEvents:ends.length};
  const start=BigInt(starts[0].atNs),finish=BigInt(ends[0].atNs);
  if(finish<start)return {state:begin==='retirement-requested' && end==='original-process-io-ended'
    ?'exit-before-retirement-request':'reverse-observation-order',startNs:String(start),endNs:String(finish)};
  return {state:'measured',ms:Number(finish-start)/1e6,startNs:String(start),endNs:String(finish)};
}
export function analyzeRetainedLifecycle(statuses) {
  const owners=new Map();let invalidEvents=0;
  for(const status of statuses.filter(Boolean)) {
    for(const bundle of status.retainedWorker?.callSamples ?? []) {
      const context=[status.sessionEpoch,status.state?.activePlanVersion,bundle.nodeIds];
      const key=JSON.stringify(context);
      if(!owners.has(key))owners.set(key,{context,events:new Map(),maxReportedDrops:0});
      const owner=owners.get(key);
      owner.maxReportedDrops=Math.max(owner.maxReportedDrops,bundle.calls.phaseDropped ?? 0);
      for(const event of bundle.calls.phases ?? []) {
        if(event.atNs!==null && (typeof event.atNs!=='string' || !/^\d{1,20}$/.test(event.atNs))) {invalidEvents++;continue;}
        owner.events.set(JSON.stringify(event),event);
      }
    }
  }
  const result={scope:'Host observations within each evaluation cohort; not OS receipts',invalidEvents,owners:[]};
  for(const owner of owners.values()) {
    const events=[...owner.events.values()].sort((a,b)=>a.atNs===null?1:b.atNs===null?-1:
      BigInt(a.atNs)<BigInt(b.atNs)?-1:BigInt(a.atNs)>BigInt(b.atNs)?1:0);
    const attempts=[...new Set(events.map(event=>event.attempt).filter(id=>id!==null))];
    const item={context:owner.context,observedEvents:events.length,maxReportedDrops:owner.maxReportedDrops,
      unknownAttemptEvents:events.filter(event=>event.attempt===null).length,attempts:[],invocations:[]};
    for(const attempt of attempts) {
      const own=events.filter(event=>event.attempt===attempt);
      item.attempts.push({attempt,processIds:[...new Set(own.map(e=>e.processId).filter(pid=>pid!==null))],
        spans:Object.fromEntries(Object.entries(pairs).map(([name,[a,b]])=>[name,interval(own,a,b)]))});
    }
    const runs=[...new Set(events.map(event=>event.runId).filter(id=>id!==null))];
    for(const runId of runs) {
      const own=events.filter(event=>event.runId===runId);
      item.invocations.push({runId,offeredToDequeued:interval(own,'invocation-offered','invocation-dequeued'),
        offeredToCall:interval(own,'invocation-offered','invocation-call-started'),
        observedCall:interval(own,'invocation-call-started','invocation-call-finished')});
    }
    result.owners.push(item);
  }
  return result;
}
