// Normal External Host, verified packages, actual consent/Core and AppContainer.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {createHash} from 'node:crypto';
import {readFileSync, writeFileSync, mkdtempSync, rmSync, realpathSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, resolve, dirname} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {measureHostInput} from './host-input-scenarios.mjs';

const [hostArg, managerArg, workerArg, dllArg, officialArg, countArg='2', samplesArg='10', shape='identity', cadence='warm'] = process.argv.slice(2);
assert(officialArg, 'HOST MANAGER WORKER DLL OFFICIAL COUNT SAMPLES identity|mixed warm|paced');
const [hostExe, managerExe, worker, dll, official] = [hostArg,managerArg,workerArg,dllArg,officialArg].map(value=>resolve(value));
const count=Number(countArg), samples=Number(samplesArg);
assert([1,2,4,8,16,20].includes(count) && Number.isInteger(samples) && samples>=1 && samples<=1000);
assert(['identity','mixed'].includes(shape) && ['warm','paced'].includes(cadence));
const mode=process.env.DIY_EXPERIMENT_REVIEWED_PRIMITIVE_RETENTION ?? '0';
assert(['0','separate','grouped'].includes(mode));
const reportPath=process.env.DIY_PRIMITIVE_RETENTION_REPORT;
assert(reportPath, 'explicit report path required');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const parent=realpathSync(tmpdir()), root=mkdtempSync(join(parent,'diyime-primitive-retention-'));
const kinds=shape==='identity' ? ['demo.identity'] : ['text.romaji-to-hiragana','experimental.text-trim','experimental.text-case','experimental.packet-passthrough'];
const ids=Array.from({length:count},(_,i)=>`stage${i}`);
const kindAt=i=>shape==='identity' ? kinds[0] : i===0 ? kinds[0] : kinds[1+(i-1)%3];
const graph={version:1,nodes:[{id:'input',kind:'input.text',version:1},...ids.map((id,i)=>({id,kind:kindAt(i),version:1}))],
  edges:ids.map((id,i)=>({from:{node:i ? ids[i-1] : 'input',port:'out'},to:{node:id,port:'in'}}))};
const target={node:ids.at(-1),port:'out'};
const contract=JSON.parse(readFileSync(new URL('../tests/contracts/execution_control.json',import.meta.url),'utf8'));
const resourcePolicy={...contract.resourceProfiles.bounded,maxPhysicalJobs:22,maxQueuedJobs:32,
  maxRetainedBytes:67108864,maxResultBytesPerRun:16384,retentionMs:60000,maxRetainedRuns:32,maxDedupeEntries:32,maxTombstones:32};
const packageManager={officialRoot:official,dataRoot:join(root,'data'),workspace:join(root,'workers'),worker,runtimeDll:dll,
  policy:{processMemoryBytes:536870912,wasmMemoryBytes:134217728,fuelPerCall:50000000,walltimeMs:30000,terminationGraceMs:5}};
const managerPath=join(root,'manager.json'), hostPath=join(root,'host.json');
writeFileSync(managerPath,JSON.stringify(packageManager));
writeFileSync(hostPath,JSON.stringify({packageManager,resourcePolicy,permissionRoot:join(root,'permissions'),requestBytes:1048576,responseBytes:16777216}));

class Client {
  constructor(exe,path) {
    this.next=0;this.pending=new Map();this.stderr=[];
    this.child=spawn(exe,[path],{windowsHide:true,stdio:'pipe'});
    this.child.stderr.on('data',data=>this.stderr.push(data));
    this.ended=new Promise(done=>this.child.once('close',code=>{
      for(const p of this.pending.values())p.reject(Error(`child exit ${code}`));done(code);
    }));
    createInterface({input:this.child.stdout}).on('line',line=>{
      const reply=JSON.parse(line),pending=this.pending.get(reply.id);
      assert(pending,'unexpected protocol reply');this.pending.delete(reply.id);pending.resolve(reply);
    });
  }
  async send(operation,payload=Buffer.alloc(0)) {
    const id=String(++this.next),body=Buffer.from(JSON.stringify({id,operation})),header=Buffer.alloc(12);
    header.writeUInt32LE(body.length);header.writeBigUInt64LE(BigInt(payload.length),4);
    const result=new Promise((resolve,reject)=>this.pending.set(id,{resolve,reject}));
    this.child.stdin.write(Buffer.concat([header,body,payload]));return result;
  }
  async call(operation,payload) { const r=await this.send(operation,payload);assert.equal(r.ok,true,JSON.stringify(r.error));return r.data; }
  async close() {this.child.stdin.end();assert.equal(await this.ended,0,Buffer.concat(this.stderr).toString());}
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
const expected=text=>shape==='identity' ? text : count===1 ? text.replace('ka','か') : text.replace('ka','か').trim();
function verify(s,wanted) {
  assert.equal(s.current,true);assert.equal(s.result.reason,'completed',JSON.stringify(s.result.error));assert.equal(s.result.delivery,'complete');
  const value=s.result.ports.find(p=>p.endpoint.node===target.node && p.endpoint.port==='out')?.value;
  assert.equal(value?.packet?.root.text,wanted);assert(ended(s));
}
let manager,host,revision=0,sequence=0;
const records=[],traces=[],statuses=[];
const report={boundary:'normal External Host consent/Core/AppContainer; client input through terminal and zero input callbacks',
  mode,count,samples,shape,cadence,resourcePolicy,graph,records,traces,statuses,
  requestedControls:Object.fromEntries(Object.entries(process.env).filter(([key])=>key.startsWith('DIY_EXPERIMENT_'))),
  binaries:Object.fromEntries([['host',hostExe],['manager',managerExe],['worker',worker],['dll',dll]].map(([k,p])=>[k,sha(readFileSync(p))])),
  moduleSha256:sha(readFileSync(join(official,'official.wasm'))),completed:false};
try {
  manager=new Client(managerExe,managerPath);
  const frames=[...new Set(ids.map((_,i)=>kindAt(i)))].map(kind=>{
    const manifest=readFileSync(join(official,kind+'.manifest.json')),module=readFileSync(join(official,'official.wasm')),header=Buffer.alloc(24);
    header.write('DIY2NODE');header.writeUInt32LE(1,8);header.writeUInt32LE(manifest.length,12);header.writeBigUInt64LE(BigInt(module.length),16);
    const frame=Buffer.concat([header,manifest,module]),size=Buffer.alloc(8);size.writeBigUInt64LE(BigInt(frame.length));return Buffer.concat([size,frame]);
  });
  const frameCount=Buffer.alloc(4);frameCount.writeUInt32LE(frames.length);
  const review=await manager.call({method:'review'},Buffer.concat([frameCount,...frames]));
  await manager.call({method:'commit',reviewId:review.reviewId,allowDowngrade:false,allowSameVersionRebuild:false});
  await manager.close();manager=undefined;
  host=new Client(hostExe,hostPath);
  const preparation=performance.now(),pending=await host.call({method:'prepare',graph});
  await host.call({method:'approve',reviewId:pending.reviewId,ids:pending.missing.map(r=>r.id),save:false});
  const activated=await host.call({method:'activate',reviewId:pending.reviewId});
  const version=activated.state.activePlanVersion;report.prepareAndActivateMs=performance.now()-preparation;
  if(mode!=='0') {
    const ready=await until(async()=>{const s=await host.call({method:'status'});report.lastReadinessStatus=s;return s.retainedWorker.ready?s:null;});
    assert.equal(ready.retainedWorker.experimental,true);
    const sizes=mode==='separate'?ids.map(()=>1):Array.from({length:Math.ceil(count/4)},(_,i)=>Math.min(4,count-i*4));
    assert.deepEqual(ready.retainedWorker.groupSizes,sizes);assert.equal(ready.retainedWorker.processIds.length,sizes.length);
    assert.equal(new Set(ready.retainedWorker.processIds).size,sizes.length);
    assert.equal(ready.retainedWorker.physicalJobs,sizes.length);assert.equal(ready.workerProcesses,sizes.length);
    report.lifetimeReady=ready;report.prepareThroughReadinessMs=performance.now()-preparation;
  }
  async function submit(text) {
    ++revision;if(revision>1)await host.call({method:'invalidateInput',expectedPlanVersion:version,revision});
    const reply=await host.send({method:'run',request:{startKey:String(++sequence),expectedPlanVersion:version,targets:[target],input:{revision,text,mode:'compose'}}});
    if(!reply.ok){assert.equal(reply.error?.code,'busy');return {admissionRejected:'busy'};}return reply.data.receipt.key;
  }
  const read=async key=>(await host.call({method:'getRun',key})).snapshot;
  if(cadence==='paced') {
    const steps=Array.from({length:7},(_,i)=>{const text=`  ka${i}🙂漢字  `;return {id:String(i),text,expected:expected(text)};});
    report.pacing=await measureHostInput({steps,submit,read,ended,verify,summarize,pollMs:1,rounds:samples,independentInput:true});
  } else {
    for(let sample=0;sample<=samples;sample++) {
      if(sample)await delay(100);
      await host.call({method:'traceStartDetailed'});
      const text=`  ka${sample}🙂漢字  `,started=performance.now(),key=await submit(text);
      assert(!key.admissionRejected,'sequential admission rejected');
      const snapshot=await until(async()=>{const s=await read(key);return ended(s)?s:null;});
      const resultMs=performance.now()-started;verify(snapshot,expected(text));
      if(sample===0)report.prepareThroughFirstResultMs=performance.now()-preparation;
      let after='0',events=[];
      for(;;){const {trace}=await host.call({method:'traceRead',after,limit:256});assert.equal(trace.dropped,0);events.push(...trace.events);if(!trace.events.length)break;after=trace.events.at(-1).sequence;}
      await host.call({method:'traceStop'});
      traces.push({sample,key,events});records.push({sample,resultMs});
      const status=await host.call({method:'status'});statuses.push(status);
      if(mode!=='0'){assert.equal(status.retainedWorker.decodedReplies,(sample+1)*count);assert.equal(status.workerProcesses,report.lifetimeReady.workerProcesses);}
    }
    report.warmResultMs=summarize(records.slice(1).map(r=>r.resultMs));
  }
  report.beforeClose=await host.call({method:'status'});
  report.closeAcknowledged=await host.call({method:'closeSession'});
  // Dedupe/tombstone ledger entries intentionally outlive a closed Session.
  // Require all execution/IO and buffer owners to settle, without treating
  // bounded control records as surviving processes or weakening their expiry.
  const ownedFields=['activeRuns','physicalJobs','queuedJobs','queuedInputBytes','reservedResultBytes'];
  report.afterClose=await until(async()=>{const s=await host.call({method:'status'});return s.workerProcesses===0 && ownedFields.every(k=>s.physical[k]===0)?s:null;});
  await host.close();host=undefined;report.completed=true;
} catch(error) {report.failure={message:String(error),stack:error.stack};throw error;}
finally {
  try {if(manager)await manager.close();if(host)await host.close();}
  finally {
    writeFileSync(reportPath,JSON.stringify(report));
    assert.equal(dirname(realpathSync(root)),parent);assert(root.startsWith(join(parent,'diyime-primitive-retention-')));
    rmSync(root,{recursive:true});
  }
}
console.log(JSON.stringify({mode,count,shape,cadence,completed:report.completed,warm:report.warmResultMs}));
