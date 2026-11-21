// Normal External Host, verified packages, actual consent/Core and AppContainer.
import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {createHash} from 'node:crypto';
import {readFileSync, writeFileSync, mkdtempSync, rmSync, realpathSync, readdirSync, writeSync} from 'node:fs';
import {tmpdir,cpus,totalmem,release,platform,arch,availableParallelism} from 'node:os';
import {join, resolve, dirname} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {measureHostInput} from './host-input-scenarios.mjs';
import {analyzeLinearCallTiming} from './analyze-retained-call-timing.mjs';
import {analyzeRetainedLifecycle} from './analyze-retained-lifecycle.mjs';
import {decodeTracePage} from '../editor/src/executionTrace.ts';
import {HeldPrimitiveWorker} from './hold-owned-primitive-worker.mjs';
import {PrivateReplyEvidence} from './private-sample-delivery.mjs';
import {wasmCodeSha256} from './wasm-host-overhead.mjs';
import {retainedComposition,fixtureDefinition,fixtureKinds} from './retained-composition-graphs.mjs';
import {analyzeRetainedGraphCalls,verifyGraphCallCoverage} from './analyze-retained-graph-calls.mjs';
import {observeWarmInput,warmInputDenominators} from './host-warm-input-observation.mjs';

const [hostArg, managerArg, workerArg, dllArg, officialArg, countArg='2', samplesArg='10', shape='identity', cadence='warm'] = process.argv.slice(2);
assert(officialArg, 'HOST MANAGER WORKER DLL OFFICIAL COUNT SAMPLES identity|mixed|thirdparty warm|paced');
const [hostExe, managerExe, worker, dll, official] = [hostArg,managerArg,workerArg,dllArg,officialArg].map(value=>resolve(value));
const count=Number(countArg), samples=Number(samplesArg);
assert([1,2,4,8,16,20].includes(count) && Number.isInteger(samples) && samples>=1 && samples<=1000);
assert(['identity','mixed','thirdparty'].includes(shape) && ['warm','paced','safety','cold','cold-paced','startup-refusal'].includes(cadence));
const cold=cadence==='cold' || cadence==='cold-paced' || cadence==='startup-refusal';
const paced=cadence==='paced' || cadence==='cold-paced';
const mode=process.env.DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION ?? '0';
const timing=process.env.DIY_EXPERIMENT_RETAINED_TIMING==='1';
const sampleDeliveryCheck=process.env.DIY_PRIMITIVE_SAMPLE_DELIVERY_CHECK==='1';
const sampleDeliveryObserve=sampleDeliveryCheck || process.env.DIY_PRIMITIVE_SAMPLE_DELIVERY_OBSERVE==='1';
const tracePaced=timing || process.env.DIY_PRIMITIVE_COMPARE_TIMING==='1';
assert(['0','separate','grouped'].includes(mode));
const reportPath=process.env.DIY_PRIMITIVE_RETENTION_REPORT;
assert(reportPath, 'explicit report path required');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const parent=realpathSync(tmpdir()), root=mkdtempSync(join(parent,'diyime-primitive-retention-'));
const thirdparty=shape==='thirdparty';
const layout=process.env.DIY_PRIMITIVE_LAYOUT??'serial';
const matrixMode=process.env.DIY_PRIMITIVE_MATRIX==='1';
const metadataComparison=process.env.DIY_PRIMITIVE_METADATA_PREPARATION_COMPARISON==='1';
if(metadataComparison) assert(matrixMode && thirdparty && mode==='separate' && layout==='serial' && cadence==='cold-paced');
assert(layout==='serial'||matrixMode,'additional layouts require explicit matrix mode');
assert(thirdparty||layout==='serial','additional layouts require the typed fixture matrix');
const composition=thirdparty?retainedComposition(count,layout):null;
const reviewedModules=thirdparty ? ['DIY_REVIEW_MODULE_A','DIY_REVIEW_MODULE_B'].map(name=>readFileSync(resolve(process.env[name]))) : [];
if(thirdparty) {
  assert.notEqual(wasmCodeSha256(reviewedModules[0]),wasmCodeSha256(reviewedModules[1]),'reviewed fixtures must differ in executable code');
  assert(['0','separate'].includes(mode),'S1 qualification does not grant grouped S2 approval');
}
const kinds=thirdparty ? fixtureKinds : shape==='identity' ? ['demo.identity'] : ['text.romaji-to-hiragana','experimental.text-trim','experimental.text-case','experimental.packet-passthrough'];
const ids=Array.from({length:count},(_,i)=>`stage${i}`);
// The passthrough advertises an `any` output root. It may be terminal,
// but cannot feed a text-only input. Preserve the published contracts.
// demo.identity advertises Any output; keep it terminal before text-only fixtures.
const kindAt=i=>thirdparty ? composition.kinds[i] : shape==='identity' ? kinds[0] : i===0 ? kinds[0]
  : i===count-1 && count>=4 ? kinds[3] : kinds[1+(i-1)%2];
const graph=composition?.graph??{version:1,nodes:[{id:'input',kind:'input.text',version:1},...ids.map((id,i)=>({id,kind:kindAt(i),version:1,...(thirdparty&&kindAt(i)!==kinds[2]?{config:{mode:'forward'}}:{})}))],
  edges:ids.map((id,i)=>({from:{node:i ? ids[i-1] : 'input',port:'out'},to:{node:id,port:'in'}}))};
const targets=composition?.targets??[{node:ids.at(-1),port:'out'}];
const target=targets.at(-1);
const executedIds=composition?.executedIds??ids;
const executedCount=executedIds.length;
const contract=JSON.parse(readFileSync(new URL('../tests/contracts/execution_control.json',import.meta.url),'utf8'));
const resourcePolicy={...contract.resourceProfiles.bounded,maxPhysicalJobs:composition?.maxPhysicalJobs??22,maxQueuedJobs:32,
  maxRetainedBytes:67108864,maxResultBytesPerRun:matrixMode?524288:16384,retentionMs:60000,maxRetainedRuns:32,maxDedupeEntries:32,maxTombstones:32};
const packageManager={officialRoot:official,dataRoot:join(root,'data'),workspace:join(root,'workers'),worker,runtimeDll:dll,
  policy:{processMemoryBytes:536870912,wasmMemoryBytes:134217728,fuelPerCall:50000000,walltimeMs:30000,terminationGraceMs:5}};
const managerPath=join(root,'manager.json'), hostPath=join(root,'host.json');
writeFileSync(managerPath,JSON.stringify(packageManager));
writeFileSync(hostPath,JSON.stringify({packageManager,resourcePolicy,permissionRoot:join(root,'permissions'),requestBytes:1048576,responseBytes:16777216}));

const journalEnabled=process.env.DIY_PRIMITIVE_PHASE_JOURNAL==='1';
const nativeStartupTiming=process.env.DIY_EXPERIMENT_RETAINED_NATIVE_STARTUP_TIMING==='1';
let journalRemaining=512;
function journal(role,event,method,pid) {
  if(!journalEnabled || journalRemaining<=0)return;
  journalRemaining--;
  writeSync(2,JSON.stringify({diagnostic:'retained-benchmark-phase',elapsedMs:performance.now(),role,event,method,pid})+'\n');
}

class Client {
  constructor(exe,path,role) {
    this.role=role;this.lastSent=undefined;this.lastReply=undefined;
    this.next=0;this.pending=new Map();this.stderr=[];this.received=[];
    this.replyEvidence=new PrivateReplyEvidence();
    this.child=spawn(exe,[path],{windowsHide:true,stdio:'pipe'});
    journal(this.role,'spawn',undefined,this.child.pid);
    this.child.stderr.on('data',data=>this.stderr.push(data));
    this.ended=new Promise(done=>this.child.once('close',code=>{
      journal(this.role,'exit',undefined,this.child.pid);
      for(const p of this.pending.values())p.reject(Error(`child exit ${code}`));done(code);
    }));
    createInterface({input:this.child.stdout}).on('line',line=>{
      const reply=JSON.parse(line),pending=this.pending.get(reply.id);
      if(cadence==='safety')this.received.push(reply);
      assert(pending,'unexpected protocol reply');this.pending.delete(reply.id);pending.resolve(reply);
      if(this.lastReply!==pending.method)journal(this.role,'reply',pending.method,this.child.pid);
      this.lastReply=pending.method;
      this.replyEvidence.record(pending.method,line,reply);
    });
  }
  async send(operation,payload=Buffer.alloc(0)) {
    const id=String(++this.next),body=Buffer.from(JSON.stringify({id,operation})),header=Buffer.alloc(12);
    header.writeUInt32LE(body.length);header.writeBigUInt64LE(BigInt(payload.length),4);
    const result=new Promise((resolve,reject)=>this.pending.set(id,{resolve,reject,method:operation.method}));
    if(this.lastSent!==operation.method)journal(this.role,'send',operation.method,this.child.pid);
    this.lastSent=operation.method;
    this.child.stdin.write(Buffer.concat([header,body,payload]));return result;
  }
  async call(operation,payload) { const r=await this.send(operation,payload);assert.equal(r.ok,true,JSON.stringify(r.error));return r.data; }
  async close() {
    journal(this.role,'close-stdin',undefined,this.child.pid);
    this.child.stdin.end();assert.equal(await this.ended,0,Buffer.concat(this.stderr).toString());
    const size=Math.min(65536,this.stderr.reduce((sum,part)=>sum+part.length,0));
    const lines=Buffer.concat(this.stderr,size).toString().split(/\r?\n/);
    const audit=lines.filter(line=>
      /^host-trace-loss-audit total=\d{1,20} probeBusy=\d{1,20} reservationFull=\d{1,20} clockUnavailable=\d{1,20} recorderBusy=\d{1,20} hostQueueFull=\d{1,20} invalidNative=\d{1,20} unfinished=\d{1,20}$/.test(line)).slice(0,64);
    if(audit.length)report.traceLossAudit=[...(report.traceLossAudit ?? []),...audit].slice(0,64);
    if(nativeStartupTiming && this.role==='host') {
      const records=lines.flatMap(line=>{
        const match=/^retained-native-startup pid=(\d{1,10}) verify_us=(\d{1,16}) load_us=(\d{1,16}) read_us=(\d{1,16}) restore_us=(\d{1,16})$/.exec(line);
        if(!match)return [];
        const [pid,dllVerifyUs,dllLoadUs,moduleReadUs,moduleRestoreUs]=match.slice(1).map(Number);
        if(pid<=0 || ![pid,dllVerifyUs,dllLoadUs,moduleReadUs,moduleRestoreUs].every(Number.isSafeInteger))return [];
        return [{pid,dllVerifyUs,dllLoadUs,moduleReadUs,moduleRestoreUs}];
      });
      report.retainedNativeStartup={records:records.slice(0,256),truncated:size===65536||records.length>256};
    }
  }
}
async function until(check) {
  const end=performance.now()+120000;
  for(;;){const value=await check();if(value)return value;assert(performance.now()<end,'bounded observation expired');await delay(1);}
}
const summarize=values=>{
  assert(values.length);const sorted=[...values].sort((a,b)=>a-b),p=q=>sorted[Math.ceil(sorted.length*q)-1];
  return {count:values.length,p50:p(.5),p95:p(.95),p99:p(.99),max:sorted.at(-1)};
};
const ended=s=>s.state==='terminal' && Object.values(s.physical).every(n=>n===0);
const expected=text=>thirdparty ? composition.expected(text) : shape==='identity' ? text : count===1 ? text.replace('ka','か') : text.replace('ka','か').trim();
function verify(s,wanted) {
  assert.equal(s.current,true);assert.equal(s.result.reason,'completed',JSON.stringify(s.result.error));assert.equal(s.result.delivery,'complete');
  for(const target of targets) {
    const value=s.result.ports.find(p=>p.endpoint.node===target.node&&p.endpoint.port===target.port)?.value;
    if(composition)composition.verify(value?.packet?.root.text,wanted);
    else assert.equal(value?.packet?.root.text,wanted,`wrong target ${target.node}.${target.port}`);
  }
}
let manager,host,revision=0,sequence=0;
const records=[],traces=[],statuses=[],warmAttempts=[];
const report={syntheticOnly:true,boundary:'normal External Host consent/Core/AppContainer; client input through terminal and zero input callbacks',
  sourceCommit:process.env.GITHUB_SHA ?? process.env.DIY_MEASUREMENT_SOURCE_SHA ?? null,
  runtimeBuildSourceCommit:process.env.GITHUB_SHA ?? process.env.DIY_MEASUREMENT_BUILD_SOURCE_SHA ?? null,
  measurementStartedUtc:new Date().toISOString(),
  machine:{osPlatform:platform(),osRelease:release(),architecture:arch(),logicalCpus:cpus().length,
    availableParallelism:availableParallelism(),cpuModels:[...new Set(cpus().map(cpu=>cpu.model))],totalPhysicalMemoryBytes:totalmem()},
  mode,count,samples,shape,cadence,layout,matrixMode,executedIds,unexecutedIds:composition?.unexecutedIds??[],resourcePolicy,graph,records,traces,statuses,warmAttempts,tracePaced,sampleDeliveryCheck,sampleDeliveryObserve,readinessBarrierUsed:mode!=='0' && !cold,
  ...(timing ? {goalOverheadAvailable:false,semanticSeparation:'Guest API includes SDK ABI processing; residuals are NOT Node Execution Overhead.'} : {}),
  requestedControls:Object.fromEntries(['HOST_TRACE_LOSS_AUDIT','RETAINED_STARTUP_PARALLELISM','RETAINED_TIMING','RETAINED_COMPILED_JIT','REVIEWED_PRIMITIVE_RETENTION','PREPARED_JIT','INHERITED_CODE_ACL','VERIFIED_RUNTIME_FILE','OVERLAP_WORKER_CLEANUP','COMPILED_JIT','POOLED_JIT','JIT_STREAM_MODULE','NEUTRAL_JIT_POOL','NEUTRAL_CODE_CACHE','NEUTRAL_CODE_MAPPING'].map(name=>['DIY_EXPERIMENT_'+name,process.env['DIY_EXPERIMENT_'+name] ?? null])),
  binaries:Object.fromEntries([['host',hostExe],['manager',managerExe],['worker',worker],['dll',dll]].map(([k,p])=>[k,sha(readFileSync(p))])),
  moduleSha256:sha(readFileSync(join(official,'official.wasm'))),completed:false};
try {
  if(thirdparty)report.reviewedImplementations=reviewedModules.map(module=>({moduleSha256:sha(module),executableCodeSha256:wasmCodeSha256(module)}));
  manager=new Client(managerExe,managerPath,'manager');
  const frames=[...new Set(ids.map((_,i)=>kindAt(i)))].map(kind=>{
    const fixtureIndex=thirdparty?kinds.indexOf(kind):-1;
    const fixture=fixtureIndex>=0&&fixtureIndex!==2;
    const module=fixture?reviewedModules[fixtureIndex===3?0:fixtureIndex]:readFileSync(join(official,'official.wasm'));
    const manifest=fixture?Buffer.from(JSON.stringify({
      packageId:`test.retention-review.${fixtureIndex}`,version:'1.0.0',displayName:`Reviewed fixture ${fixtureIndex}`,
      runtime:'wasm32',guestApiVersion:1,moduleSha256:sha(module),capabilities:[],
      node:fixtureDefinition(kind)
    })):readFileSync(join(official,kind+'.manifest.json'));
    const header=Buffer.alloc(24);
    header.write('DIY2NODE');header.writeUInt32LE(1,8);header.writeUInt32LE(manifest.length,12);header.writeBigUInt64LE(BigInt(module.length),16);
    const frame=Buffer.concat([header,manifest,module]),size=Buffer.alloc(8);size.writeBigUInt64LE(BigInt(frame.length));return Buffer.concat([size,frame]);
  });
  const frameCount=Buffer.alloc(4);frameCount.writeUInt32LE(frames.length);
  const managerReviewStarted=performance.now();
  const review=await manager.call({method:'review'},Buffer.concat([frameCount,...frames]));
  await manager.call({method:'commit',reviewId:review.reviewId,allowDowngrade:false,allowSameVersionRebuild:false});
  if(metadataComparison) {
    const flag=process.env.DIY_EXPERIMENT_PREPARATION_CODE_CACHE;
    report.requestedControls.DIY_EXPERIMENT_PREPARATION_CODE_CACHE=flag??null;
    report.metadataPreparation={cacheRequested:flag==='1',managerReviewAndCommitMs:performance.now()-managerReviewStarted};
  }
  await manager.close();manager=undefined;
  host=new Client(hostExe,hostPath,'host');
  report.privateReplyEvidence=host.replyEvidence;
  const preparation=performance.now(),pending=await host.call({method:'prepare',graph});
  const prepared=performance.now();
  await host.call({method:'approve',reviewId:pending.reviewId,ids:pending.missing.map(r=>r.id),save:false});
  const approved=performance.now();
  const activated=await host.call({method:'activate',reviewId:pending.reviewId});
  const active=performance.now();
  let version=activated.state.activePlanVersion;report.prepareAndActivateMs=active-preparation;
  if(metadataComparison) Object.assign(report.metadataPreparation,{hostPrepareMs:prepared-preparation,
    hostApproveMs:approved-prepared,hostActivateMs:active-approved});
  if(mode!=='0' && !cold) {
    const ready=await until(async()=>{const s=await host.call({method:'status'});report.lastReadinessStatus=s;assert.equal(s.phase,'ready','evaluation Session was retired during startup');assert.equal(s.sessionEpoch,activated.sessionEpoch,'evaluation Session changed during startup');assert.equal(s.retainedWorker.startupFailures ?? 0,0,'retained startup failed before all groups became ready');return s.retainedWorker.ready?s:null;});
    assert.equal(ready.retainedWorker.experimental,true);
    const sizes=mode==='separate'?ids.map(()=>1):Array.from({length:Math.ceil(count/4)},(_,i)=>Math.min(4,count-i*4));
    assert.deepEqual(ready.retainedWorker.groupSizes,sizes);assert.equal(ready.retainedWorker.processIds.length,sizes.length);
    assert.equal(new Set(ready.retainedWorker.processIds).size,sizes.length);
    assert.equal(ready.retainedWorker.physicalJobs,sizes.length);assert.equal(ready.workerProcesses,sizes.length);
    report.lifetimeReady=ready;report.prepareThroughReadinessMs=performance.now()-preparation;
    if(process.env.DIY_EXPERIMENT_RETAINED_COMPILED_JIT==='1') {
      assert.equal(mode,'separate');
      assert.equal(ready.retainedWorker.compiledGenerations,ready.retainedWorker.generation,'compiled replacement route must actually be selected');
    }
  }
  async function submit(text,observation) {
    ++revision;if(revision>1)await host.call({method:'invalidateInput',expectedPlanVersion:version,revision});
    const reply=await host.send({method:'run',request:{startKey:String(++sequence),expectedPlanVersion:version,targets,input:{revision,text,mode:'compose'}}});
    if(observation) {
      observation.admitted=typeof reply.ok==='boolean'?reply.ok:null;
      if(!reply.ok)observation.admissionRejected=reply.error?.code??'unknown-rejection';
    }
    if(!reply.ok){assert.equal(reply.error?.code,'busy');return {admissionRejected:'busy'};}return reply.data.receipt.key;
  }
  const read=async key=>(await host.call({method:'getRun',key})).snapshot;
  async function collectTrace() {
    // Freeze after all input callbacks have settled. Paginating a live trace
    // takes its mutex and can itself cause diagnostic try_lock contention.
    // Stop also drains the worker inbox before we inspect its final loss count.
    const {trace:stopped}=await host.call({method:'traceStop'});
    decodeTracePage(stopped);
    const collection={recordingId:stopped.recordingId ?? null,dropped:stopped.dropped,eventCount:0};
    (report.traceCollections ??= []).push(collection);
    assert.equal(stopped.recording,false);
    let after='0',events=[];
    for(;;){
      const {trace}=await host.call({method:'traceRead',after,limit:256});
      decodeTracePage(trace);
      assert.equal(trace.recording,false);
      collection.dropped=Math.max(collection.dropped,trace.dropped);
      events.push(...trace.events);
      collection.eventCount=events.length;
      if(!trace.events.length)break;
      after=trace.events.at(-1).sequence;
    }
    if(collection.dropped)collection.partialEvents=events;
    assert.equal(collection.dropped,0,'frozen trace has missing events');
    return events;
  }
  const observedNativeHistory=[];
  function analyze(keys,events,status) {
    const analysis=layout==='serial'
      ?analyzeLinearCallTiming({keys,events,status,ids:executedIds,kinds:executedIds.map(id=>kindAt(ids.indexOf(id)))})
      :analyzeRetainedGraphCalls({keys,status,executedIds,unexecutedIds:composition.unexecutedIds,history:observedNativeHistory});
    assert.equal(analysis.correlationError,null);
    assert.equal(analysis.inconsistent,0,'same-call durations must be internally consistent');
    if(layout!=='serial') {
      const admitted=keys.filter(key=>key?.runId);
      verifyGraphCallCoverage(analysis,admitted.at(-1)?.runId);
      observedNativeHistory.push(status);
    }
    return analysis;
  }
  async function verifySampleRetrieval(status,requiredNodeIds=timing && mode!=='0'?executedIds:[]) {
    // Outside the measured input/trace window, after all callbacks settled.
    // Explicit reads must retain the complete samples AND their loss counters.
    if(sampleDeliveryObserve) {
      const repeated=await host.call({method:'status'});
      if(!timing || mode==='0')assert.equal(Object.hasOwn(status.retainedWorker,'callSamples'),false,'timing OFF/default must not expose private samples');
      host.replyEvidence.verifyRepeatedStatus(status,repeated,requiredNodeIds);
      if(sampleDeliveryCheck)host.replyEvidence.assertExclusive();
    }
  }
  if(cadence==='startup-refusal') {
    assert.equal(mode,'separate');
    assert(['2','4'].includes(process.env.DIY_EXPERIMENT_RETAINED_STARTUP_PARALLELISM));
    assert.equal(process.env.DIY_EXPERIMENT_COMPILED_JIT,'0');
    const key=await submit('ka');
    assert(!key.admissionRejected);
    const snapshot=await until(async()=>{const s=await read(key);return ended(s)?s:null;});
    report.refusalSnapshot=snapshot;
    assert.equal(snapshot.current,true);
    assert.equal(snapshot.result.delivery,'failed','unavailable parallel route must not deliver successful S0 output');
    assert(snapshot.result.causes.some(cause=>cause.diagnostic.error.code==='retained-startup'));
    const stagePorts=snapshot.result.ports.filter(port=>ids.includes(port.endpoint.node));
    assert.equal(stagePorts.length,count);
    assert(stagePorts.every(port=>port.value.status==='error'));
    const status=await host.call({method:'status'});statuses.push(status);
    await verifySampleRetrieval(status,[]);
    assert.equal(status.retainedWorker.decodedReplies,0);
    assert.equal(status.retainedWorker.compiledGenerations,0);
    assert.equal(status.workerProcesses,0);
    report.startupRefusalVerified=true;
  } else if(paced) {
    const steps=Array.from({length:7},(_,i)=>{const text=`  ka${i}🙂漢字  `;return {id:String(i),text,expected:expected(text)};});
    report.pacedRounds=[];
    report.pacing=await measureHostInput({steps,submit,read,ended,verify,summarize,pollMs:1,rounds:samples,independentInput:true,
      intervals:cold ? [20,100,null] : [null,100,20],
      beginRound:tracePaced ? async()=>{await host.call({method:'traceStartDetailed'});} : undefined,
      observeRound:round=>report.pacedRounds.push(round),
      finishRound:async(intervalMs,attempts,round)=>{
        if(metadataComparison && intervalMs===20 && round.round===0) {
          const latest=attempts.at(-1);
          assert.equal(latest.outcome,'correct');
          report.metadataPreparation.firstLatestResponseMs=latest.responseMs;
          report.metadataPreparation.hostPrepareThroughFirstLatestMs=latest.issued+latest.responseMs-preparation;
        }
        const status=await host.call({method:'status'});
        if(metadataComparison && status.retainedWorker.ready &&
            report.metadataPreparation.hostPrepareThroughReadinessObservationMs===undefined) {
          report.metadataPreparation.hostPrepareThroughReadinessObservationMs=performance.now()-preparation;
        }
        // Preserve correct/failed/latest outcomes even if diagnostics fail.
        round.status=status;
        const events=tracePaced ? await collectTrace() : undefined;
        await verifySampleRetrieval(status);
        const paired=timing ? analyze(attempts.map(a=>a.key),events,status) : undefined;
        Object.assign(round,...(events?[{events}]:[]),...(paired?[{sameCall:paired}]:[]));
        if(paired)assert.equal(paired.rows.filter(row=>row.key.runId===attempts.at(-1).key.runId && row.status==='paired').length,executedCount,'latest correct input needs a complete same-call measurement');
        if(process.env.DIY_EXPERIMENT_RETAINED_COMPILED_JIT==='1')assert.equal(status.retainedWorker.compiledGenerations,status.retainedWorker.generation,'replacement silently used raw compilation');
        assert.equal(attempts.at(-1).outcome,'correct','latest input must complete correctly');
      }});
    for(const scenario of report.pacing) {
      assert.equal(scenario.attempts,steps.length*samples);
      assert.equal(scenario.failed,scenario.admissionRejected,'unexpected execution failure');
      assert.equal(scenario.timedOut,0);assert.equal(scenario.cancelled,0);
      assert.equal(scenario.correct+scenario.superseded+scenario.admissionRejected,scenario.attempts);
      assert.equal(scenario.byCase['6'].correct,samples);
    }
  } else {
    if(timing)report.sameCall=[];
    for(let sample=0;sample<=samples;sample++) {
      if(sample)await delay(100);
      await host.call({method:'traceStartDetailed'});
      const text=`  ka${sample}🙂漢字  `;
      const {key,snapshot,resultMs}=await observeWarmInput({sample,attempts:warmAttempts,
        submit:attempt=>submit(text,attempt),read,until,ended,verify,expected:expected(text)});
      if(sample===0)report.prepareThroughFirstResultMs=performance.now()-preparation;
      const events=await collectTrace();
      traces.push({sample,key,events});records.push({sample,resultMs});
      const status=await host.call({method:'status'});statuses.push(status);
      await verifySampleRetrieval(status);
      if(timing) {
        const paired=analyze([key],events,status);
        report.sameCall.push({sample,...paired});
        assert.equal(paired.paired,executedCount,'correct graph run needs every actual Native measurement');
      }
      if(sample===0)report.firstResultStatus=status;
      if(mode!=='0') {
        assert.equal(status.retainedWorker.decodedReplies,(sample+1)*executedCount);
        if(!cold)assert.equal(status.workerProcesses,report.lifetimeReady.workerProcesses);
        if(process.env.DIY_EXPERIMENT_RETAINED_COMPILED_JIT==='1')assert.equal(status.retainedWorker.compiledGenerations,status.retainedWorker.generation);
      }
    }
    report[cold?'followupResultMs':'warmResultMs']=summarize(records.slice(1).map(r=>r.resultMs));
    if(cold)report.followupIsGuaranteedWarm=false;
    if(timing) {
      if(layout==='serial')report[cold?'followupResidualMs':'warmResidualMs']=summarize(report.sameCall.filter(s=>s.sample>0).flatMap(s=>s.rows.filter(r=>r.status==='paired').map(r=>r.nodeResidualMs)));
    }
  }
  if(cadence==='safety') {
    assert.notEqual(mode,'0','retention safety requires an explicit retained route');
    report.safety=[];
    async function correct(text) {
      const key=await submit(text);assert(!key.admissionRejected);
      const snapshot=await until(async()=>{const s=await read(key);return ended(s)?s:null;});
      verify(snapshot,expected(text));return {key,snapshot};
    }
    async function drained() {
      return until(async()=>{const s=await host.call({method:'status'});return s.workerProcesses===0 && s.physical.physicalJobs===0 && s.physical.reservedResultBytes===0?s:null;});
    }
    let latestReview=pending;
    async function activateFresh(nextGraph=graph) {
      latestReview=await host.call({method:'prepare',graph:nextGraph});
      if(latestReview.missing.length)await host.call({method:'approve',reviewId:latestReview.reviewId,ids:latestReview.missing.map(r=>r.id),save:false});
      const activated=await host.call({method:'activate',reviewId:latestReview.reviewId});
      version=activated.state.activePlanVersion;revision=0;
      return until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.ready?s:null;});
    }
    await correct('');await correct('🙂漢字');
    report.safety.push({check:'empty-and-unicode',passed:true});
    const old=await correct('ka');await activateFresh();
    assert.equal((await host.send({method:'getRun',key:old.key})).ok,false);
    await correct('ka');report.safety.push({check:'plan-session-replacement-and-old-key-fence',passed:true});

    let observedCancelled=0;
    const beforeControlled=await until(async()=>{
      const status=await host.call({method:'status'});
      return status.retainedWorker.ready&&!status.retainedWorker.activeInput?status:null;
    });
    statuses.push(beforeControlled);
    const controlledPid=beforeControlled.retainedWorker.processIds[0];assert(controlledPid>0);
    const controlledPython=process.env.DIY_PRIMITIVE_PYTHON;assert(controlledPython);
    const control=new HeldPrimitiveWorker({python:controlledPython,pid:controlledPid,
      workspace:join(root,'workers'),digest:report.binaries.worker});
    const proof={scope:'actual Host/IPC input against a controlled suspended Worker; not guest-running proof',
      pid:controlledPid,passed:false};
    report.controlledCancellation=proof;
    try {
      proof.held=await control.started;
      const key=await submit('0'.repeat(256));assert(!key.admissionRejected);
      proof.key=key;
      const active=await until(async()=>{
        const status=await host.call({method:'status'});
        const snapshot=await read(key);
        assert(!ended(snapshot),'held Worker input completed before controlled cancellation');
        return status.retainedWorker.activeInput&&snapshot.physical.running>0?{status,snapshot}:null;
      });
      proof.observedActive=true;proof.activePhysical=active.snapshot.physical;
      await host.call({method:'cancelRun',key});
      // Wait for the helper to close its dead-process/thread references before
      // requiring Host ACL/profile reclamation. Extra observer handles can
      // otherwise obstruct that reclamation and change the experiment.
      proof.ending=await control.expectOriginalExit();
      const snapshot=await until(async()=>{const value=await read(key);return ended(value)?value:null;});
      assert.equal(snapshot.result.reason,'cancelled');
      proof.outcome=snapshot.result.reason;proof.finalPhysical=snapshot.physical;
      observedCancelled++;
      await correct('ka');
      const restored=await until(async()=>{const status=await host.call({method:'status'});return status.retainedWorker.ready?status:null;});
      statuses.push(restored);
      assert(restored.retainedWorker.generation>beforeControlled.retainedWorker.generation);
      assert.equal(restored.retainedWorker.physicalJobs,beforeControlled.retainedWorker.physicalJobs);
      assert.equal(restored.workerProcesses,beforeControlled.workerProcesses);
      assert.equal(restored.physical.physicalJobs,beforeControlled.physical.physicalJobs);
      proof.freshGeneration=restored.retainedWorker.generation;proof.passed=true;
      report.safety.push({check:'controlled-active-host-input-cancel-original-exit-and-recovery',passed:true});
    } finally {proof.helperCompletion=await control.close();}
    for(let trial=0;trial<3;trial++) {
      const key=await submit('0'.repeat(256));assert(!key.admissionRejected);
      const observation=await until(async()=>{
        const status=await host.call({method:'status'});if(status.retainedWorker.activeInput)return {active:true,status};
        const snapshot=await read(key);return ended(snapshot)?{active:false,snapshot}:null;
      });
      if(observation.active)await host.call({method:'cancelRun',key});
      const snapshot=await until(async()=>{const s=await read(key);return ended(s)?s:null;});
      if(snapshot.result.reason==='cancelled')observedCancelled++;
      else verify(snapshot,expected('0'.repeat(256)));
      await correct('ka');
      report.safety.push({check:'active-cancel-and-recovery',trial,observedActive:observation.active,outcome:snapshot.result.reason});
    }
    assert(observedCancelled>0,'must observe actual active-input cancellation');

    const marker='秘密検査🔒',held=await submit(marker.repeat(30));
    const query=host.send({method:'getRun',key:held}),protection=host.send({method:'protection',enabled:true}),ack=String(host.next);
    await Promise.all([query,protection]);
    const index=host.received.findIndex(r=>r.id===ack);assert(index>=0);
    assert(!host.received.slice(index+1).some(r=>JSON.stringify(r).includes(marker)));
    const protectedStatus=await drained();assert.equal(protectedStatus.phase,'protected');
    assert.equal((await host.send({method:'getRun',key:held})).ok,false);
    await host.call({method:'protection',enabled:false});await activateFresh();await correct('ka');
    report.safety.push({check:'protection-response-fence-original-process-drain-and-recovery',passed:true});

    const revoked=await correct('ka');
    const execute=latestReview.requests.find(r=>r.kind==='execute');assert(execute);
    await host.call({method:'revoke',id:execute.id});
    assert.equal((await host.send({method:'getRun',key:revoked.key})).ok,false);
    const revokedStatus=await drained();assert.equal(revokedStatus.phase,'empty');
    await activateFresh();await correct('ka');
    report.safety.push({check:'revocation-drain-consent-and-recovery',passed:true});

    const beforeFault=await correct('ka'),status=await host.call({method:'status'});
    const pid=status.retainedWorker.processIds.at(-1);assert(pid>0);
    const python=process.env.DIY_PRIMITIVE_PYTHON;assert(python,'explicit Python for original owned-worker fault required');
    const fault=spawnSync(python,[resolve('scripts/terminate-owned-retention-worker.py'),String(pid),join(root,'workers'),report.binaries.worker],{encoding:'utf8',windowsHide:true,timeout:10000});
    assert.equal(fault.status,0,fault.stderr);verify(await read(beforeFault.key),expected('ka'));
    await correct('ka');
    const recovered=await until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.ready?s:null;});
    assert(!recovered.retainedWorker.processIds.includes(pid));
    report.safety.push({check:'owned-original-process-fault-no-retroactive-result-change-and-recovery',passed:true,pid});

    const successor=await correct('ka');await host.call({method:'closeSession'});await drained();
    assert.equal((await host.send({method:'getRun',key:successor.key})).ok,false);
    await activateFresh();await correct('ka');
    report.safety.push({check:'close-session-and-fresh-reopen',passed:true});
  }
  report.beforeClose=await host.call({method:'status'});
  report.closeAcknowledged=await host.call({method:'closeSession'});
  // Dedupe/tombstone ledger entries intentionally outlive a closed Session.
  // Require all execution/IO and buffer owners to settle, without treating
  // bounded control records as surviving processes or weakening their expiry.
  const ownedFields=['activeRuns','physicalJobs','queuedJobs','queuedInputBytes','reservedResultBytes'];
  report.afterClose=await until(async()=>{const s=await host.call({method:'status'});return s.workerProcesses===0 && ownedFields.every(k=>s.physical[k]===0)?s:null;});
  if(sampleDeliveryObserve) {
    if(sampleDeliveryCheck)host.replyEvidence.assertExclusive();
    assert(host.replyEvidence.repeatedSnapshotsChecked>0,'no settled explicit retrieval was checked');
    assert(host.replyEvidence.byMethod.getRun?.replies>0,'no ordinary result reply was checked');
  }
  await host.close();host=undefined;
  function remainingWorkspaceFiles(folder) {
    let entries;
    try {entries=readdirSync(folder,{withFileTypes:true});}
    catch(error){if(error.code==='ENOENT')return [];throw error;}
    return entries.flatMap(entry=>{
      const path=join(folder,entry.name);
      return entry.isDirectory() && !entry.isSymbolicLink() ? remainingWorkspaceFiles(path) : [path];
    });
  }
  // Inspect BEFORE test-directory removal: that removal must not hide an
  // execution/code-cache leak after the original Host and Workers have exited.
  journal('driver','check-workspace');
  report.workspaceAfterHostExit=remainingWorkspaceFiles(join(root,'workers'));
  assert.deepEqual(report.workspaceAfterHostExit,[],'Host must reclaim workspace assets itself');
  report.completed=true;
} catch(error) {report.failure={message:String(error),stack:error.stack};throw error;}
finally {
  if(!paced&&cadence!=='startup-refusal')report.warmInputDenominators=warmInputDenominators(warmAttempts,samples+1);
  if(timing)report.lifecycle=analyzeRetainedLifecycle([report.lastReadinessStatus,report.lifetimeReady,
    ...statuses,...(report.pacedRounds ?? []).map(round=>round.status),report.beforeClose]);
  try {if(manager)await manager.close();if(host)await host.close();}
  finally {
    writeFileSync(reportPath,JSON.stringify(report));
    assert.equal(dirname(realpathSync(root)),parent);assert(root.startsWith(join(parent,'diyime-primitive-retention-')));
    journal('driver','remove-fixture');
    rmSync(root,{recursive:true});
    journal('driver','fixture-removed');
  }
}
console.log(JSON.stringify({mode,count,shape,cadence,completed:report.completed,warm:report.warmResultMs}));
