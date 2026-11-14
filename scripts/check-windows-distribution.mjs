import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {readFileSync,mkdtempSync,rmSync} from 'node:fs';
import {resolve,join,dirname} from 'node:path';
import {tmpdir} from 'node:os';
import {createHash} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import assert from 'node:assert/strict';
const [folder, option, ...extra]=process.argv.slice(2);
if(!folder||extra.length||(option!==undefined&&option!=='--authority-regression'))throw Error('usage: BUILT_APPLICATION_FOLDER [--authority-regression]');
const root=mkdtempSync(join(tmpdir(),'diyime-app-smoke-')),data=join(root,'data'),manifest=JSON.parse(readFileSync(join(folder,'bundle-manifest.json'),'utf8'));
for(const file of manifest.files){assert(!file.path.includes('..')&&!file.path.includes('\\')&&!file.path.startsWith('/'));const bytes=readFileSync(join(folder,file.path));assert.equal(bytes.length,file.bytes);assert.equal(createHash('sha256').update(bytes).digest('hex'),file.sha256);}
let child,url,beat,closed;
async function until(test,ms=240000){const end=Date.now()+ms;while(Date.now()<end){const value=await test();if(value)return value;await delay(30);}throw Error('application check timed out');}
async function request(path,operation,method='POST'){const response=await fetch(url+path,{method,headers:{'Content-Type':'application/json',Origin:url.slice(0,-1)},body:method==='GET'||method==='DELETE'?undefined:JSON.stringify(operation??{})});return {response,value:await response.json()};}
async function call(path,operation,method){const {response,value}=await request(path,operation,method);assert(response.ok,JSON.stringify(value));assert.equal(value.ok,true);return value.data;}
async function start(){const t=performance.now();child=spawn(join(folder,'DIY_IME2.exe'),['--no-browser','--data-root',data,'--port','0'],{stdio:'pipe',windowsHide:true,env:{...process.env,PATH:''}});child.stderr.on('data',()=>{});closed=new Promise(resolve=>child.once('close',code=>resolve(code)));const lines=createInterface({input:child.stdout});url=await new Promise((resolve,reject)=>{lines.once('line',line=>resolve(JSON.parse(line).url));child.once('close',code=>reject(Error('launcher exited '+code)));child.once('error',reject);});beat=setInterval(()=>{void request('api/app/heartbeat',{}).catch(()=>{});},1000);const first=await until(async()=>{const response=await fetch(url+'api/app');const state=await response.json();if(state.phase==='failed')throw Error('application setup failed');return state.phase==='ready'?state:null;});assert.equal(first.prepared,48);return Math.round(performance.now()-t);}
async function stop(){clearInterval(beat);const {response,value}=await request('api/app/close',{});assert(response.ok);assert.equal(value.closed,true);assert.equal(await closed,0);child=undefined;}
try{
 const firstBootMs=await start();assert((await(await fetch(url)).text()).includes('/assets/'));assert.equal((await fetch(url+'api/nodes')).status,403);let connection=(await call('api/external/connections',{})).connectionId;
 const host=operation=>call('api/external/connections/'+connection+'/request',operation);
 const definitions=(await host({method:'definitions'})).definitions;assert.equal(definitions.length,52);
 assert(!manifest.files.some(file=>file.path.includes('test-host')), 'synthetic Host must never ship');
 const inventory=JSON.parse(readFileSync(new URL('../docs/node-migration-inventory.json',import.meta.url),'utf8'));
 const expectedDefinitions=inventory.nodes.filter(node=>node.registered).map(node=>node.definition);
 for(const definition of definitions)assert.deepEqual(definition,expectedDefinitions.find(expected=>expected.kind===definition.kind),'captured definition '+definition.kind);
 const graph=await call('api/examples/official-dictionary',undefined,'GET'),first=await host({method:'prepare',graph});assert.equal(first.requests.length,9);assert.equal(first.missing.length,9);
 await host({method:'approve',reviewId:first.reviewId,ids:first.missing.filter(r=>r.kind!=='resourceRead').map(r=>r.id),save:true});const denied=await request('api/external/connections/'+connection+'/request',{method:'activate',reviewId:first.reviewId});assert.equal(denied.value.error.code,'permission-required');
 const review=await host({method:'prepare',graph});assert.deepEqual(review.missing.map(r=>r.kind),['resourceRead']);await host({method:'approve',reviewId:review.reviewId,ids:review.missing.map(r=>r.id),save:true});await host({method:'activate',reviewId:review.reviewId});
 const runMs=[];
 for(const [text,expected,revision] of [['kousui','降水',1],['honjitudesu','ほんじつです',2],['30%','30%',3],['','',4]]){if(revision>1)await host({method:'invalidateInput',expectedPlanVersion:'1',revision});const t=performance.now(),{receipt}=await host({method:'run',request:{startKey:String(revision),expectedPlanVersion:'1',targets:[{node:'result',port:'out'}],input:{revision,text,mode:'compose'}}});const snapshot=await until(async()=>{const {snapshot}=await host({method:'getRun',key:receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});const value=snapshot.result?.ports.find(p=>p.endpoint.node==='result')?.value;assert.equal(value?.status,'data',JSON.stringify(snapshot));assert.deepEqual(value.packet,{version:1,root:{type:'text',text:expected}});runMs.push(Math.round(performance.now()-t));}
 // Preserve saved demo/E2E/practical graphs through the actual official module, not Native test fixtures.
 let compatibilityRuns=0;
 async function activateSample(name,mutate){
  const candidate=await call('api/examples/'+name,undefined,'GET');if(mutate)mutate(candidate);
  const review=await host({method:'prepare',graph:candidate});
  if(review.missing.length)await host({method:'approve',reviewId:review.reviewId,ids:review.missing.map(item=>item.id),save:false});
  const activated=await host({method:'activate',reviewId:review.reviewId});assert.equal(activated.state.activePlanVersion,'1');return candidate;
 }
 async function compatibilityRun(targets,text='',revision=1,mode='finalize'){
  if(revision>1)await host({method:'invalidateInput',expectedPlanVersion:'1',revision});
  const {receipt}=await host({method:'run',request:{startKey:String(++compatibilityRuns),expectedPlanVersion:'1',targets,input:{revision,text,mode}}});
  const snapshot=await until(async()=>{const {snapshot}=await host({method:'getRun',key:receipt.key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(value=>value===0)?snapshot:null;});
  assert(snapshot.result,JSON.stringify(snapshot));return snapshot.result;
 }
 const port=(result,node,id='out')=>{const value=result.ports.find(port=>port.endpoint.node===node&&port.endpoint.port===id)?.value;assert(value,JSON.stringify(result));return value;};
 const textPacket=(result,node,expected,id='out')=>assert.deepEqual(port(result,node,id),{status:'data',packet:{version:1,root:{type:'text',text:expected}}});
 await activateSample('hello');textPacket(await compatibilityRun([{node:'result',port:'out'}]),'result','HELLO 𠮷');
 await activateSample('branch');const branch=await compatibilityRun([{node:'selected',port:'out'},{node:'inactive',port:'out'}]);textPacket(branch,'selected','HELLO');assert.deepEqual(port(branch,'inactive'),{status:'no-output'});
 for(const [name,expected]of [['weather','本日の降水確率は30%です'],['weather-negative','本日の降水率は30%です'],['stringify','本日の降水確率は30%です']]){
  await activateSample(name);textPacket(await compatibilityRun([{node:'output',port:'out'},{node:'result',port:'out'}]),'result',expected);
 }
 await activateSample('practical-pipeline');textPacket(await compatibilityRun([{node:'result',port:'out'}],'honjitunokousuikakurituha30%desu'),'result','本日の降水確率は30%です');
 await activateSample('runtime-romaji');let revision=0;
 for(const [text,mode,out,pending]of [['k','compose','','k'],['ky','compose','','ky'],['kya','compose','きゃ',''],['ky','compose','','ky'],['','compose','',''],['n','compose','','n'],['n','finalize','ん',''],['𠮷30%desu','finalize','𠮷30%です','']]){
  const result=await compatibilityRun([{node:'romaji',port:'out'},{node:'romaji',port:'pending'}],text,++revision,mode);textPacket(result,'romaji',out);textPacket(result,'romaji',pending,'pending');assert.equal((await host({method:'status'})).state.activePlanVersion,'1');
 }
 await activateSample('dictionary-exact');revision=0;
 for(const [text,expected]of [['kousui','降水'],['honjitudesu','ほんじつです'],['30%','30%'],['','']])textPacket(await compatibilityRun([{node:'result',port:'out'}],text,++revision),'result',expected);
 await activateSample('runtime-romaji',candidate=>{candidate.nodes.find(node=>node.id==='romaji').config={overrides:[],unknown:'error'};});
 const failure=await compatibilityRun([{node:'romaji',port:'out'}],'q');assert.equal(port(failure,'romaji').status,'error');assert(failure.causes.length>0);
 console.log(JSON.stringify({kind:'official-wasm-cutover-compatibility',definitions:52,compatibilityRuns,legacyGraphs:true,romajiSnapshots:8,dictionarySnapshots:4,noOutput:true,error:true,physicalEnding:true}));
 await call('api/external/connections/'+connection,undefined,'DELETE');await stop();
 const restartMs=await start();connection=(await call('api/external/connections',{})).connectionId;const reopened=await host({method:'prepare',graph});assert.equal(reopened.missing.length,0);await host({method:'activate',reviewId:reopened.reviewId});
 if(option==='--authority-regression'){
  const endpoint='api/external/connections/'+connection+'/request';
  const startRequest=(startKey,revision)=>({method:'run',request:{startKey:String(startKey),expectedPlanVersion:'1',targets:[{node:'result',port:'out'}],input:{revision,text:'kousui',mode:'compose'}}});
  const workerRunning=()=>until(async()=> (await host({method:'status'})).workerProcesses>0,10000);
  const drained=()=>until(async()=>{const status=await host({method:'status'});return status.workerProcesses===0&&status.physical.physicalJobs===0?status:null;},10000);
  const refused=async (operation,code)=>{const {response,value}=await request(endpoint,operation);assert(!response.ok);assert.equal(value.ok,false);assert.equal(value.error?.code,code);};
  const cancelled=(await host(startRequest(1,1))).receipt;
  await workerRunning();
  await host({method:'cancelRun',key:cancelled.key});
  await drained();
  const {snapshot}=await host({method:'getRun',key:cancelled.key});
  assert.equal(snapshot.state,'terminal');
  assert(Object.values(snapshot.physical).every(value=>value===0));
  assert.equal(snapshot.result?.reason,'cancelled');
  await host({method:'invalidateInput',expectedPlanVersion:'1',revision:2});
  const protectedRun=(await host(startRequest(2,2))).receipt;
  await workerRunning();
  await host({method:'protection',enabled:true});
  assert.equal((await drained()).phase,'protected');
  await refused({method:'getRun',key:protectedRun.key},'external-protected');
  await refused(startRequest(3,3),'external-protected');
  await host({method:'protection',enabled:false});
  const restored=await host({method:'prepare',graph});
  assert.equal(restored.missing.length,0);
  await host({method:'activate',reviewId:restored.reviewId});
  const revokedRun=(await host(startRequest(1,1))).receipt;
  await workerRunning();
  const subject=restored.requests.find(item=>item.kind==='execute');
  assert(subject);
  await host({method:'revoke',id:subject.id});
  assert.equal((await drained()).phase,'empty');
  await refused({method:'getRun',key:revokedRun.key},'external-not-ready');
  await refused(startRequest(2,2),'external-not-ready');
  console.log(JSON.stringify({kind:'built-windows-authority-regression',cancelDuringWorker:true,protectDuringWorker:true,revokeDuringWorker:true,physicalEnding:true}));
 }
 await stop();
 // A crashed launcher must not orphan the privileged server/Host or isolated Workers.
 const crashData=join(root,'crash-data');child=spawn(join(folder,'DIY_IME2.exe'),['--no-browser','--data-root',crashData,'--port','0'],{stdio:'pipe',windowsHide:true,env:{...process.env,PATH:''}});child.stderr.on('data',()=>{});closed=new Promise(resolve=>child.once('close',code=>resolve(code)));const crashLines=createInterface({input:child.stdout});url=await new Promise((resolve,reject)=>{crashLines.once('line',line=>resolve(JSON.parse(line).url));child.once('close',code=>reject(Error('crash probe launch '+code)));});
 const rootPid=child.pid,system=process.env.SystemRoot;
 async function allProcesses(){const script='@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CreationDate,Name) | ConvertTo-Json -Compress';const p=spawn(join(system,'System32/WindowsPowerShell/v1.0/powershell.exe'),['-NoProfile','-NonInteractive','-EncodedCommand',Buffer.from(script,'utf16le').toString('base64')],{windowsHide:true,stdio:['ignore','pipe','pipe']});let output='';p.stdout.on('data',b=>output+=b);p.stderr.on('data',()=>{});assert.equal(await new Promise(resolve=>p.once('close',resolve)),0);const value=JSON.parse(output||'[]');return Array.isArray(value)?value:[value];}
 function descendants(all){const ids=new Set([rootPid]);for(let i=0;i<all.length;i++){let changed=false;for(const p of all)if(ids.has(p.ParentProcessId)&&!ids.has(p.ProcessId)){ids.add(p.ProcessId);changed=true;}if(!changed)break;}return all.filter(p=>p.ProcessId!==rootPid&&ids.has(p.ProcessId));}
 const observed=await until(async()=>{const current=descendants(await allProcesses());return current.some(p=>p.Name==='worker.exe')?current:null;},30000);assert(observed.some(p=>p.Name==='node.exe'));assert(observed.some(p=>p.Name==='diy-ime2-node-manager.exe'));child.kill();await closed;child=undefined;
 let survivors=[];
 try{await until(async()=>{const current=await allProcesses();survivors=observed.filter(old=>current.some(p=>p.ProcessId===old.ProcessId&&p.CreationDate===old.CreationDate));return survivors.length===0;},30000);}
 catch(error){throw Error('launcher crash children did not end: '+JSON.stringify(survivors.map(({ProcessId,ParentProcessId,CreationDate,Name})=>({ProcessId,ParentProcessId,CreationDate,Name}))),{cause:error});}
 console.log(JSON.stringify({kind:'built-windows-application',files:manifest.files.length,firstBootMs,restartMs,runMs,officialPackages:48,definitions:52,explicitConsent:true,partialConsentRefused:true,savedReopen:true,unicodeTieUnknownEmpty:true,noPathToolchain:true,trueClose:true,launcherCrashChildrenEnded:true}));
}finally{clearInterval(beat);if(child){try{await stop();}catch{child.kill();await closed;}}if(dirname(root)!==resolve(tmpdir())||!root.startsWith(join(tmpdir(),'diyime-app-smoke-')))throw Error('unsafe cleanup');rmSync(root,{recursive:true});}
