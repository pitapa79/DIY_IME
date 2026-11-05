import {decodeTracePage} from '../editor/src/executionTrace.ts';
import {spawn,spawnSync} from 'node:child_process';
import {mkdtempSync,readFileSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {dirname,join,resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {createInterface} from 'node:readline';
import assert from 'node:assert/strict';
import {setTimeout as delay} from 'node:timers/promises';
const [exe,producer,worker,dll,wasm,resourceFile]=process.argv.slice(2);
if(!resourceFile)throw Error('usage: check-external-host.mjs HOST PACKAGE_TOOL WORKER DLL WASM RESOURCE_CONTRACT');
const root=mkdtempSync(join(tmpdir(),'diyime-external-host-'));
const dataRoot=join(root,'data'),permissionRoot=join(root,'permissions'),workspace=join(root,'workers');
const module=readFileSync(wasm);
const definition={kind:'fixture.wasm',version:1,inputs:[{port:{id:'in',dataType:{id:'text-packet',version:1},language:'ja-Latn',root:'text'},required:true}],outputs:[{id:'out',dataType:{id:'text-packet',version:1},language:null,root:'text'}],configSchema:{type:'object'},defaultConfig:{mode:'echo'}};
function command(args){const result=spawnSync(producer,args,{encoding:'utf8',windowsHide:true});assert.equal(result.status,0,result.stderr);return result.stdout;}
function pack(bytes,name){
 const input=join(root,name+'.wasm'),manifest=join(root,name+'.json'),output=join(root,name+'.diynode');
 writeFileSync(input,bytes);writeFileSync(manifest,JSON.stringify({packageId:'test.external',version:'1.0.0',displayName:'External fixture',runtime:'wasm32',guestApiVersion:1,node:definition,moduleSha256:createHash('sha256').update(bytes).digest('hex'),capabilities:[]}));
 command(['pack',manifest,input,output]);return output;
}
const packageFile=pack(module,'original');
command(['import',dataRoot,join(root,'install-workers'),worker,dll,packageFile,'--commit']);
const policy=JSON.parse(readFileSync(resourceFile,'utf8')).resourceProfiles.bounded;
policy.maxPhysicalJobs=1;policy.maxQueuedJobs=0;
// Retain real-process test results while CI schedules subsequent assertions.
policy.retentionMs=60000;
const config=join(root,'config.json');
writeFileSync(config,JSON.stringify({packageManager:{dataRoot,workspace,worker,runtimeDll:dll,policy:{processMemoryBytes:536870912,wasmMemoryBytes:134217728,fuelPerCall:null,walltimeMs:null,terminationGraceMs:5}},permissionRoot,resourcePolicy:policy,requestBytes:1048576,responseBytes:16777216}));
class Client{
 constructor(){
  this.child=spawn(exe,[config],{windowsHide:true,stdio:'pipe'});this.child.stderr.resume();this.next=0;this.pending=new Map();this.replies=[];
  this.exited=new Promise(resolve=>this.child.once('close',(code,signal)=>{for(const p of this.pending.values())p.reject(Error('Host exited'));this.pending.clear();resolve({code,signal});}));
  createInterface({input:this.child.stdout}).on('line',line=>{const reply=JSON.parse(line);this.replies.push(reply);const pending=this.pending.get(reply.id);if(pending){this.pending.delete(reply.id);pending.resolve(reply);}});
 }
 send(operation){
  const id=String(++this.next),json=Buffer.from(JSON.stringify({id,operation})),header=Buffer.alloc(12);header.writeUInt32LE(json.length);header.writeBigUInt64LE(0n,4);
  const result=new Promise((resolve,reject)=>this.pending.set(id,{resolve,reject}));this.child.stdin.write(header);this.child.stdin.write(json);result.requestId=id;return result;
 }
 async call(operation){const reply=await this.send(operation);assert.equal(reply.ok,true,JSON.stringify(reply.error));return reply.data;}
 async close(){this.child.stdin.end();const exit=await this.exited;assert.equal(exit.code,0);}
}
const graph=(mode='echo')=>({version:1,nodes:[{id:'source',kind:'input.text',version:1},{id:'worker',kind:'fixture.wasm',version:1,config:{mode}}],edges:[{from:{node:'source',port:'out'},to:{node:'worker',port:'in'}}]});
const inputText='private-runtime-input-canary';
const startRequest=()=>({startKey:'1',expectedPlanVersion:'1',targets:[{node:'worker',port:'out'}],input:{revision:1,text:inputText,mode:'compose'}});
async function until(check,diagnostic=async()=>({})){const end=Date.now()+60000;for(;;){const result=await check();if(result)return result;if(Date.now()>end)throw Error('Host did not settle: '+JSON.stringify(await diagnostic()));await delay(20);}}
async function approve(client,mode,save=false){
 const review=await client.call({method:'prepare',graph:graph(mode)});
 if(review.missing.length)await client.call({method:'approve',reviewId:review.reviewId,ids:review.missing.map(r=>r.id),save});
 const active=await client.call({method:'activate',reviewId:review.reviewId});assert.equal(active.phase,'ready');return {review,active};
}
async function execute(client){
 const {receipt}=await client.call({method:'run',request:startRequest()});
 let last;
 const value=await until(async()=>{const {snapshot}=await client.call({method:'getRun',key:receipt.key});last={state:snapshot.state,current:snapshot.current,physical:snapshot.physical};return snapshot.state==='terminal'?snapshot:null;},async()=>({snapshot:last,status:await client.call({method:'status'})}));
 const text=value.result.ports.find(p=>p.endpoint.node==='worker').value.packet.root.text;
 assert.equal(JSON.parse(text).operation.inputs.in.packet.root.text,inputText);
 return receipt.key;
}
let client;
try{
 client=new Client();
 const initial=await client.call({method:'status'});assert.equal(initial.phase,'empty');assert.equal(initial.workerProcesses,0);
 const definitions=await client.call({method:'definitions'});assert.equal(definitions.definitions.length,5);
 assert(!definitions.definitions.some(d=>d.kind==='text.romaji-to-hiragana')); // No legacy/native processing fallback.
 assert.equal((await client.send({method:'run',request:startRequest()})).ok,false);
 const invalid=await client.send({method:'status',trusted:true,path:'private'});assert.equal(invalid.ok,false);
 assert.equal((await client.call({method:'status'})).phase,'empty');
 // Canonical system-only plans do not acquire fictitious external permissions.
 const systemGraph={version:1,nodes:[{id:'source',kind:'input.text',version:1}],edges:[]};
 const systemReview=await client.call({method:'prepare',graph:systemGraph});assert.equal(systemReview.requests.length,0);assert.equal(systemReview.missing.length,0);
 await client.call({method:'activate',reviewId:systemReview.reviewId});
 const offTrace=decodeTracePage((await client.call({method:'traceRead',after:'0',limit:256})).trace);assert.equal(offTrace.recording,false);assert.equal(offTrace.recordingId,null);
 assert.equal((await client.send({method:'traceRead',after:'00',limit:1})).ok,false);assert.equal((await client.send({method:'traceRead',after:'0',limit:257})).ok,false);
 const startedTrace=decodeTracePage((await client.call({method:'traceStart'})).trace);assert.equal(startedTrace.recording,true);
 const systemRun=await client.call({method:'run',request:{...startRequest(),targets:[{node:'source',port:'out'}]}});
 const systemResult=await until(async()=>{const {snapshot}=await client.call({method:'getRun',key:systemRun.receipt.key});return snapshot.state==='terminal'?snapshot:null;});
 const systemTrace=decodeTracePage((await client.call({method:'traceRead',after:'0',limit:256})).trace);
 assert(systemTrace.events.some(e=>e.event==='run-accepted'));assert(systemTrace.events.some(e=>e.event==='core-result-ready')||systemTrace.dropped>0);assert(systemTrace.events.some(e=>e.event==='host-response-published')||systemTrace.dropped>0);
 const systemJson=JSON.stringify(systemTrace);assert(!systemJson.includes(inputText));assert(!systemJson.includes(systemRun.receipt.key.sessionEpoch));
 const stoppedTrace=decodeTracePage((await client.call({method:'traceStop'})).trace);assert.equal(stoppedTrace.recording,false);
 const clearedTrace=decodeTracePage((await client.call({method:'traceClear'})).trace);assert.equal(clearedTrace.recordingId,null);assert.equal(clearedTrace.events.length,0);
 assert.equal(systemResult.result.ports[0].value.packet.root.text,inputText);assert.equal((await client.call({method:'status'})).workerProcesses,0);
 await client.call({method:'protection',enabled:true});assert.equal((await client.send({method:'getRun',key:systemRun.receipt.key})).ok,false);
 await client.call({method:'protection',enabled:false});assert.equal((await client.send({method:'getRun',key:systemRun.receipt.key})).ok,false);
 await client.call({method:'closeSession'});
 const first=await client.call({method:'prepare',graph:graph()});assert.equal(first.missing.length,2);
 assert.equal((await client.send({method:'approve',reviewId:first.reviewId,ids:['f'.repeat(64)],save:true})).ok,false);
 await client.call({method:'approve',reviewId:first.reviewId,ids:first.missing.map(r=>r.id),save:false});
 await client.call({method:'activate',reviewId:first.reviewId});
 await client.call({method:'traceStart'});
 const oldKey=await execute(client);
 const wasmTrace=decodeTracePage((await client.call({method:'traceRead',after:'0',limit:256})).trace);assert(wasmTrace.events.some(e=>e.event==='node-started')||wasmTrace.dropped>0);assert(wasmTrace.events.some(e=>e.event==='call-ended')||wasmTrace.dropped>0);assert(!JSON.stringify(wasmTrace).includes(inputText));assert(!JSON.stringify(wasmTrace).includes(oldKey.sessionEpoch));
 assert(!wasmTrace.events.some(e=>/^(host-request|worker-staging|worker-process|worker-cleanup|broker-exchange)-/.test(e.event)), 'basic recording must not collect Host details');
 assert(!wasmTrace.events.some(e=>e.worker!==undefined||e.event==='worker-interval'), 'basic recording must not collect Worker intervals');

 const replay=await client.call({method:'run',request:startRequest()});assert.equal(replay.receipt.replayed,true);
 assert.deepEqual((await client.call({method:'getStart',startKey:'1'})).key,oldKey);
 assert(!JSON.stringify(await client.call({method:'status'})).includes(inputText));
 await client.call({method:'invalidateInput',expectedPlanVersion:'1',revision:2});
 const stale=await client.call({method:'getRun',key:oldKey});assert.equal(stale.snapshot.current,false);assert.equal(stale.snapshot.result,null);
 await client.call({method:'closeSession'});
 const second=await client.call({method:'prepare',graph:graph()});assert.equal(second.missing.length,2);
 await client.call({method:'approve',reviewId:second.reviewId,ids:second.missing.map(r=>r.id),save:true});
 await client.call({method:'activate',reviewId:second.reviewId});
 assert.equal((await client.send({method:'getRun',key:oldKey})).ok,false);
 await execute(client);await client.close();client=new Client();
 const restored=await client.call({method:'prepare',graph:graph()});assert.equal(restored.missing.length,0);
 await client.call({method:'activate',reviewId:restored.reviewId});
 await execute(client);
 const held=await approve(client,'hold',false);
 const heldRun=await client.call({method:'run',request:startRequest()});
 await until(async()=>(await client.call({method:'status'})).workerProcesses>0);
 await client.call({method:'protection',enabled:true});
 assert.equal((await client.send({method:'getRun',key:heldRun.receipt.key})).ok,false);
 await until(async()=>{const s=await client.call({method:'status'});assert.equal(s.phase,'protected');return s.workerProcesses===0&&s.physical.physicalJobs===0;});
 assert.equal((await client.send({method:'protection',enabled:false,automatic:false})).ok,false);
 assert.equal((await client.call({method:'status'})).protected,true);
 await client.call({method:'protection',enabled:false});
 assert.equal((await client.send({method:'activate',reviewId:held.review.reviewId})).ok,false);
 // Race a retained payload query with protection: no Data can follow its ack.
 await client.call({method:'protection',enabled:false});
 await approve(client,'echo');
 const fencedKey=await execute(client),begin=client.replies.length;
 const query=client.send({method:'getRun',key:fencedKey}),protection=client.send({method:'protection',enabled:true});
 await Promise.all([query,protection]);
 const frames=client.replies.slice(begin),ack=frames.findIndex(r=>r.id===protection.requestId);
 assert(ack>=0);assert(!frames.slice(ack+1).some(r=>JSON.stringify(r).includes(inputText)));
 await client.call({method:'protection',enabled:false});
 const ready=await approve(client,'echo');assert.equal(ready.review.missing.length,0);
 const activeKey=await execute(client);
 const executeId=ready.review.requests.find(r=>r.kind==='execute').id;
 await client.call({method:'revoke',id:executeId});
 assert.equal((await client.send({method:'getRun',key:activeKey})).ok,false);
 const recovery=await approve(client,'echo');assert.equal(recovery.review.missing.length,1);await execute(client);
 // Prepare a new graph only after the previous actual process has ended.
 await approve(client,'hold');const beforeReplace=await client.call({method:'run',request:startRequest()});
 await until(async()=>(await client.call({method:'status'})).workerProcesses>0);
 const replacement=await approve(client,'echo');assert.equal(replacement.active.workerProcesses,0);await execute(client);
 // An implementation update invalidates even a currently running old binding.
 await approve(client,'hold');const updating=await client.call({method:'run',request:startRequest()});
 await until(async()=>(await client.call({method:'status'})).workerProcesses>0);
 const changed=Buffer.concat([module,Buffer.from([0,2,1,120])]),changedFile=pack(changed,'changed');
 command(['import',dataRoot,join(root,'install-workers'),worker,dll,changedFile,'--commit','--allow-same-version-rebuild']);
 assert.equal((await client.send({method:'getRun',key:updating.receipt.key})).ok,false);
 await until(async()=>{const status=await client.call({method:'status'});return status.workerProcesses===0&&status.physical.physicalJobs===0;});
 const update=await approve(client,'echo');assert.equal(update.review.missing.length,2);await execute(client);
 await client.call({method:'closeSession'});
 const preparing=client.send({method:'prepare',graph:graph('validateHold')});
 const busy=await client.send({method:'status'});assert.equal(busy.ok,false);assert.equal(busy.error.code,'external-busy');
 await delay(80);
 await client.call({method:'cancel',requestId:preparing.requestId});assert.equal((await preparing).ok,false);
 const cancelled=await client.call({method:'status'});assert.equal(cancelled.phase,'empty');assert.equal(cancelled.workerProcesses,0);assert.equal(cancelled.physical.physicalJobs,0);
 await approve(client,'hold');await client.call({method:'run',request:startRequest()});
 await until(async()=>(await client.call({method:'status'})).workerProcesses>0);
 await client.close();client=undefined;
 const saved=readFileSync(join(permissionRoot,'permissions.json'),'utf8');assert(!saved.includes(inputText));assert(!saved.includes('"mode"'));
 console.log('Production external Host: strict protocol, canonical registry, no execution before consent, Session/saved consent, real input flow, stale suppression, replay/query, true cancellation/protection/withdrawal, catalog change, recovery and EOF passed.');
}finally{
 if(client)await client.close();
 if(dirname(root)!==resolve(tmpdir())||!root.startsWith(join(tmpdir(),'diyime-external-host-')))throw Error('unexpected test cleanup scope');
 rmSync(root,{recursive:true});
}
