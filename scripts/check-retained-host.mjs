// Actual ordinary Host + unchanged official code. Functional proof, not a latency verdict.
import assert from 'node:assert/strict';
import {spawn,spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {existsSync,readFileSync,writeFileSync,mkdtempSync,mkdirSync,rmSync} from 'node:fs';
import {join,resolve,dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {setTimeout as delay} from 'node:timers/promises';
const [hostExe,managerExe,worker,dll,official,mode]=process.argv.slice(2);
if(!official||(mode!==undefined&&mode!=='--single-worker'))throw Error('usage: HOST MANAGER WORKER DLL OFFICIAL_DIRECTORY [--single-worker]');
const singleWorker=mode==='--single-worker';
const root=mkdtempSync(join(tmpdir(),'diyime-retained-host-'));
class Client {
 constructor(exe,config){this.serial=0;this.pending=new Map();this.child=spawn(exe,[config],{windowsHide:true,stdio:'pipe'});this.stderr=[];this.child.stderr.on('data',data=>this.stderr.push(data));this.ended=new Promise(resolve=>this.child.on('close',code=>{for(const p of this.pending.values())p.reject(Error('Host ended '+code));resolve(code);}));createInterface({input:this.child.stdout}).on('line',line=>{const value=JSON.parse(line),p=this.pending.get(value.id);if(p){this.pending.delete(value.id);p.resolve(value);}});}
 send(operation,payload=Buffer.alloc(0)){const id=String(++this.serial),json=Buffer.from(JSON.stringify({id,operation})),header=Buffer.alloc(12);header.writeUInt32LE(json.length);header.writeBigUInt64LE(BigInt(payload.length),4);const answer=new Promise((resolve,reject)=>this.pending.set(id,{resolve,reject}));this.child.stdin.write(Buffer.concat([header,json,payload]));return answer;}
 async call(operation,payload){const reply=await this.send(operation,payload);assert.equal(reply.ok,true,JSON.stringify(reply.error));return reply.data;}
 async close(){this.child.stdin.end();assert.equal(await this.ended,0,Buffer.concat(this.stderr).toString());}
}
async function until(check){const end=performance.now()+60000;for(;;){const value=await check();if(value)return value;if(performance.now()>end)throw Error('retained Host did not settle');await delay(2);}}
const kinds=['text.romaji-to-hiragana','text.dictionary-exact','text.select-candidates','text.stringify'],ids=['hiragana','dictionary','select','result'];
const graph={version:1,nodes:[{id:'input',kind:'input.text',version:1},...kinds.map((kind,i)=>({id:ids[i],kind,version:1}))],edges:[{from:{node:'input',port:'out'},to:{node:'hiragana',port:'in'}},...ids.slice(0,-1).map((node,i)=>({from:{node,port:'out'},to:{node:ids[i+1],port:'in'}}))]};
const resources=join(root,'resources');mkdirSync(resources);
const preparedDictionary=existsSync(join(official,'dictionary-producer.wasm'));
if(singleWorker&&!preparedDictionary)throw Error('single Worker check requires the official dictionary companions');
let dictionary='reading\tsurface\tweight\nこうすい\t降水\t90\nこうすい\t香水\t30\nこうすい\t洪水\t90\n';
if(preparedDictionary){
 for(let i=0;i<9997;i++){
  const row=i*7919%9997+3,key=String(row).padStart(5,'0');
  dictionary+=`試験用${key}\t合成${key}\t${row%91-45}.125\n`;
 }
}
writeFileSync(join(resources,'dictionary.tsv'),dictionary);
const packageManager={officialRoot:official,dataRoot:join(root,'data'),workspace:join(root,'workers'),worker,runtimeDll:dll,policy:{processMemoryBytes:536870912,wasmMemoryBytes:134217728,fuelPerCall:50000000,walltimeMs:30000,terminationGraceMs:5},dictionaryBindings:{dictionary:{root:resources,referencePointer:'/path',maxFileBytes:1048576,maxTotalBytes:1048576,maxFiles:4,budgetMs:1000}}};
const resourcePolicy={maxActiveRuns:4,maxPhysicalJobs:singleWorker?1:2,maxObservers:16,maxQueuedJobs:8,maxQueuedInputBytes:1048576,maxRetainedRuns:8,maxRetainedBytes:8388608,maxResultBytesPerRun:1048576,retentionMs:60000,cancelGraceMs:5,maxDedupeEntries:64,maxTombstones:64,queueWaitMs:null};
const managerPath=join(root,'manager.json'),hostPath=join(root,'host.json');writeFileSync(managerPath,JSON.stringify(packageManager));writeFileSync(hostPath,JSON.stringify({packageManager,resourcePolicy,permissionRoot:join(root,'permissions'),requestBytes:1048576,responseBytes:16777216}));
let manager,host;
try {
 manager=new Client(managerExe,managerPath);
 const frames=kinds.map(kind=>{const manifest=readFileSync(join(official,kind+'.manifest.json')),module=readFileSync(join(official,'official.wasm')),header=Buffer.alloc(24);header.write('DIY2NODE');header.writeUInt32LE(1,8);header.writeUInt32LE(manifest.length,12);header.writeBigUInt64LE(BigInt(module.length),16);const frame=Buffer.concat([header,manifest,module]),size=Buffer.alloc(8);size.writeBigUInt64LE(BigInt(frame.length));return Buffer.concat([size,frame]);});
 const count=Buffer.alloc(4);count.writeUInt32LE(frames.length);const review=await manager.call({method:'review'},Buffer.concat([count,...frames]));await manager.call({method:'commit',reviewId:review.reviewId,allowDowngrade:false,allowSameVersionRebuild:false});await manager.close();manager=undefined;
 host=new Client(hostExe,hostPath);
 const prepared=await host.call({method:'prepare',graph});assert.equal(prepared.requests.length,9);await host.call({method:'approve',reviewId:prepared.reviewId,ids:prepared.missing.map(r=>r.id),save:true});await host.call({method:'activate',reviewId:prepared.reviewId});
 if(singleWorker){
  const {receipt}=await host.call({method:'run',request:{startKey:'1',expectedPlanVersion:'1',targets:[{node:'dictionary',port:'out'}],input:{revision:1,text:'kousui',mode:'compose'}}});
  const snapshot=await until(async()=>{const {snapshot}=await host.call({method:'getRun',key:receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});
  assert.equal(snapshot.result.reason,'completed',JSON.stringify(snapshot));
  const value=snapshot.result.ports.find(p=>p.endpoint.node==='dictionary').value;
  assert.deepEqual(value.packet.root.options.map(option=>[option.value.text,option.score]),[['降水',90],['香水',30],['洪水',90]]);
  const status=await host.call({method:'status'});assert.equal(status.retainedWorker.physicalJobs,0);assert.equal(status.workerProcesses,0);
  await host.close();host=undefined;
  console.log('ordinary Host: one physical slot uses a prepared dictionary without retaining a Worker');
 }else{
 const ready=await until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.ready?s:null;});assert.equal(ready.retainedWorker.physicalJobs,1);assert(ready.retainedWorker.processId>0);assert(ready.physical.retainedBytes>=readFileSync(join(resources,'dictionary.tsv')).length);let pid=ready.retainedWorker.processId,generation=ready.retainedWorker.generation,lastKey;
 if(preparedDictionary)assert(ready.physical.retainedBytes>=729929,'the retained owner must charge the complete 10,000-row prepared index');
 console.log('retained ordinary Host ready: original isolated process observed');
 let revision=0;
 async function run(text,node='result',mode='compose') {revision++;if(revision>1)await host.call({method:'invalidateInput',expectedPlanVersion:'1',revision});const {receipt}=await host.call({method:'run',request:{startKey:String(revision),expectedPlanVersion:'1',targets:[{node,port:'out'}],input:{revision,text,mode}}});lastKey=receipt.key;const snapshot=await until(async()=>{const {snapshot}=await host.call({method:'getRun',key:receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});assert.equal(snapshot.current,true);assert.equal(snapshot.result.reason,'completed',JSON.stringify(snapshot));const status=await host.call({method:'status'});if(pid===null){pid=status.retainedWorker.processId;assert(status.retainedWorker.generation>generation);generation=status.retainedWorker.generation;}else{assert.equal(status.retainedWorker.processId,pid,'a successful input must reuse the original Worker');assert.equal(status.retainedWorker.generation,generation);}assert.equal(status.retainedWorker.physicalJobs,1);return snapshot.result.ports.find(p=>p.endpoint.node===node).value;}
 for(const [text,expected] of [['kousui','降水'],['honjitudesu','ほんじつです'],['30%','30%'],['𠮷30%desu','𠮷30%です'],['',''],['ky',''],['kya','きゃ'],['ky',''],['n','']]){const value=await run(text);assert.equal(value.status,'data');assert.equal(value.packet.root.text,expected);}
 const noInput=await host.call({method:'run',request:{startKey:String(++revision),expectedPlanVersion:'1',targets:[{node:'result',port:'out'}]}});const missing=await until(async()=>{const {snapshot}=await host.call({method:'getRun',key:noInput.receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});assert.equal(missing.result.delivery,'failed');assert.equal(missing.result.nodes.find(n=>n.nodeId==='input').status,'failed');for(const id of ids)assert.equal(missing.result.nodes.find(n=>n.nodeId===id).status,'blocked');assert.equal((await host.call({method:'status'})).retainedWorker.processId,pid);
 const kana=await run('kousui','hiragana');assert.equal(kana.packet.root.text,'こうすい');
 const candidates=await run('kousui','dictionary');assert.equal(candidates.packet.root.type,'choice');assert.deepEqual(candidates.packet.root.options.map(option=>[option.value.text,option.score]),[['降水',90],['香水',30],['洪水',90]]);
 if(preparedDictionary)console.log('retained ordinary Host: 10,000 shuffled dictionary rows prepared with 50M fuel and exact candidates');
 for(let i=0;i<20;i++){const target=i%2?'dictionary':'hiragana',value=await run('kousui',target);assert.equal(value.packet.root.type,i%2?'choice':'text');}
 const oldEpoch=(await host.call({method:'status'})).sessionEpoch,dictionaryPath=join(resources,'dictionary.tsv'),originalDictionary=readFileSync(dictionaryPath);
 writeFileSync(dictionaryPath,'reading\tsurface\tweight\nこうすい\t不正\t99\nzzz\t未一致不正\tnot-a-number\n');const broken=await host.call({method:'prepare',graph});assert.equal(broken.missing.length,0);assert.equal((await host.send({method:'activate',reviewId:broken.reviewId})).ok,false);assert.equal((await host.call({method:'status'})).sessionEpoch,oldEpoch);assert.equal((await run('kousui')).packet.root.text,'降水');writeFileSync(dictionaryPath,originalDictionary);
 const invalidGraph={...graph,version:2};assert.equal((await host.send({method:'prepare',graph:invalidGraph})).ok,false);assert.equal((await host.call({method:'status'})).sessionEpoch,oldEpoch);assert.equal((await run('kousui')).packet.root.text,'降水');
 assert.equal((await run('n','result','finalize')).packet.root.text,'ん');
 console.log('retained ordinary Host: same PID, exact final/kana/candidate targets, unknown/empty/Backspace/finalize passed');
 // Open the original process object, check its task-owned executable root, and
 // terminate only that test Worker. This is a deliberate later crash, not stop.
 const killScript="$ErrorActionPreference='Stop'; $taskWorker=Get-Process -Id ([int]$env:DIY_RETAINED_TEST_PID); $taskHandle=$taskWorker.Handle; $taskRoot=[IO.Path]::GetFullPath($env:DIY_RETAINED_TEST_ROOT)+[IO.Path]::DirectorySeparatorChar; if(-not $taskWorker.Path.StartsWith($taskRoot,[StringComparison]::OrdinalIgnoreCase)){throw 'Worker is outside test workspace'}; $taskWorker.Kill(); $taskWorker.WaitForExit()";
 const killed=spawnSync(join(process.env.SystemRoot,'System32/WindowsPowerShell/v1.0/powershell.exe'),['-NoProfile','-NonInteractive','-EncodedCommand',Buffer.from(killScript,'utf16le').toString('base64')],{windowsHide:true,encoding:'utf8',env:{...process.env,DIY_RETAINED_TEST_PID:String(pid),DIY_RETAINED_TEST_ROOT:packageManager.workspace}});assert.equal(killed.status,0,killed.stderr);
 await until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.physicalJobs===0&&s.workerProcesses===0&&s.physical.physicalJobs===0;});
 const previous=(await host.call({method:'getRun',key:lastKey})).snapshot;assert.equal(previous.current,true);assert.equal(previous.result.reason,'completed');assert.equal(previous.result.ports.find(p=>p.endpoint.node==='result').value.packet.root.text,'ん');
 pid=null;assert.equal((await run('kousui')).packet.root.text,'降水');
 console.log('retained ordinary Host: later original Worker crash kept the adopted result; replacement followed true ownership cleanup');
 let cancelled=0;
 for(let trial=0;trial<3;trial++){
  revision++;await host.call({method:'invalidateInput',expectedPlanVersion:'1',revision});const text='a'.repeat(4096),{receipt}=await host.call({method:'run',request:{startKey:String(revision),expectedPlanVersion:'1',targets:[{node:'result',port:'out'}],input:{revision,text,mode:'compose'}}});
  const observed=await until(async()=>{const s=await host.call({method:'status'});if(s.retainedWorker.activeInput)return true;const {snapshot}=await host.call({method:'getRun',key:receipt.key});return snapshot.state==='terminal'?{snapshot}:null;});
  if(observed===true)await host.call({method:'cancelRun',key:receipt.key});
  const snapshot=await until(async()=>{const {snapshot}=await host.call({method:'getRun',key:receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});
  if(snapshot.result.reason==='cancelled'){
   cancelled++;await until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.physicalJobs===0&&s.workerProcesses===0&&s.physical.physicalJobs===0;});pid=null;
  }else{assert.equal(snapshot.result.reason,'completed');assert.equal(snapshot.result.ports.find(p=>p.endpoint.node==='result').value.packet.root.text,'あ'.repeat(4096));}
  assert.equal((await run('kousui')).packet.root.text,'降水');
 }
 assert(cancelled>0,'must observe cancellation while an actual Host input is active');
 console.log('retained ordinary Host: active-input cancellation, true drain and successful replenishment passed ('+cancelled+'/3 cancelled; remaining trials completed correctly before cancellation)');
 const previousKey=lastKey;writeFileSync(dictionaryPath,'reading\tsurface\tweight\nこうすい\t香水\t99\n');const successor=await host.call({method:'prepare',graph});assert.equal((await host.call({method:'status'})).sessionEpoch,oldEpoch);await host.call({method:'activate',reviewId:successor.reviewId});assert.notEqual((await host.call({method:'status'})).sessionEpoch,oldEpoch);pid=null;generation=0;revision=0;assert.equal((await run('kousui')).packet.root.text,'香水');assert.equal((await host.send({method:'getRun',key:previousKey})).ok,false);
 writeFileSync(dictionaryPath,originalDictionary);const restore=await host.call({method:'prepare',graph});await host.call({method:'activate',reviewId:restore.reviewId});pid=null;generation=0;revision=0;assert.equal((await run('kousui')).packet.root.text,'降水');
 const preparing=host.send({method:'prepare',graph}),preparingId=String(host.serial);await host.call({method:'cancel',requestId:preparingId});assert.equal((await preparing).ok,false);assert.equal((await run('kousui')).packet.root.text,'降水');
 console.log('retained ordinary Host: missing-input failure/Blocked, failed/cancelled successor preservation, successful Session/dictionary replacement and old-key fencing passed');
 const strictGraph=JSON.parse(JSON.stringify(graph));strictGraph.nodes.find(n=>n.id==='hiragana').config={overrides:[],unknown:'error'};const strictReview=await host.call({method:'prepare',graph:strictGraph});if(strictReview.missing.length)await host.call({method:'approve',reviewId:strictReview.reviewId,ids:strictReview.missing.map(r=>r.id),save:false});await host.call({method:'activate',reviewId:strictReview.reviewId});pid=null;generation=0;revision=0;assert.equal((await run('a')).packet.root.text,'あ');
 revision++;await host.call({method:'invalidateInput',expectedPlanVersion:'1',revision});const failure=await host.call({method:'run',request:{startKey:String(revision),expectedPlanVersion:'1',targets:[{node:'result',port:'out'}],input:{revision,text:'q',mode:'compose'}}});const failed=await until(async()=>{const {snapshot}=await host.call({method:'getRun',key:failure.receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});assert.equal(failed.result.delivery,'failed');await until(async()=>{const s=await host.call({method:'status'});return s.retainedWorker.physicalJobs===0&&s.physical.physicalJobs===0&&s.workerProcesses===0;});pid=null;assert.equal((await run('a')).packet.root.text,'あ');
 console.log('retained ordinary Host: valid Node error terminated the original owner; a later correct input replenished successfully');
 await host.call({method:'protection',enabled:true});const stopped=await until(async()=>{const s=await host.call({method:'status'});return s.workerProcesses===0&&s.physical.physicalJobs===0?s:null;});assert.equal(stopped.retainedWorker.physicalJobs,0);assert.equal(stopped.retainedWorker.processId,null);assert.equal(stopped.phase,'protected');
 await host.call({method:'protection',enabled:false});const reopened=await host.call({method:'prepare',graph});assert.equal(reopened.missing.length,0);await host.call({method:'activate',reviewId:reopened.reviewId});pid=null;generation=0;revision=0;assert.equal((await run('kousui')).packet.root.text,'降水');const oldKey=lastKey,pending=await host.call({method:'prepare',graph});await host.call({method:'revoke',id:pending.requests.find(r=>r.kind==='resourceRead').id});const revoked=await host.call({method:'status'});assert.equal(revoked.phase,'empty');assert.equal(revoked.reviewId,null);assert.equal(revoked.physical.physicalJobs,0);assert.equal(revoked.workerProcesses,0);assert.equal((await host.send({method:'getRun',key:oldKey})).ok,false);
 await host.close();host=undefined;
 console.log('retained ordinary Host: protection closed the actual original owner and all reservations; Host exited');
 }
}finally{if(manager)await manager.close();if(host)await host.close();if(dirname(root)!==resolve(tmpdir())||!root.startsWith(join(tmpdir(),'diyime-retained-host-')))throw Error('unexpected cleanup root');rmSync(root,{recursive:true});}
