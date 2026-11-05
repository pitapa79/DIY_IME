import {decodeTracePage} from '../editor/src/executionTrace.ts';
import {PhaseProfiler,mergePhaseSummaries} from './trace-phase-profile.mjs';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {readFileSync,realpathSync,mkdtempSync,rmSync,lstatSync} from 'node:fs';
import {resolve,join,dirname,isAbsolute,relative} from 'node:path';
import {tmpdir,platform,arch,release} from 'node:os';
import {createHash} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {fileURLToPath} from 'node:url';
import {isDeepStrictEqual} from 'node:util';

class BenchmarkError extends Error { constructor(code){super(code);this.code=code;} }
const fail=code=>{throw new BenchmarkError(code);};
const requireValue=(condition,code)=>{if(!condition)fail(code);};
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const sourceRoot=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const physicalKeys=['running','cancelRequested','stalled','queued'];
const cases=[
 {id:'dictionary-hit',text:'kousui',expected:'降水',mode:'compose'},
 {id:'unknown-reading',text:'honjitudesu',expected:'ほんじつです',mode:'compose'},
 {id:'unicode-literal',text:'𠮷30%',expected:'𠮷30%',mode:'compose'},
 {id:'empty-data',text:'',expected:'',mode:'compose'},
 {id:'pending-prefix',text:'ky',expected:'',mode:'compose'},
 {id:'finalize-n',text:'n',expected:'ん',mode:'finalize'},
];
export function summarize(values){
 requireValue(values.length>0&&values.every(n=>Number.isFinite(n)&&n>=0),'invalid-samples');
 const sorted=[...values].sort((a,b)=>a-b),at=p=>sorted[Math.max(0,Math.ceil(sorted.length*p)-1)];
 return {count:values.length,minMs:sorted[0],p50Ms:at(.5),p95Ms:at(.95),p99Ms:at(.99),maxMs:sorted.at(-1)};
}
export function physicallyEnded(snapshot){
 requireValue(snapshot&&physicalKeys.every(k=>Number.isSafeInteger(snapshot.physical?.[k])&&snapshot.physical[k]>=0),'invalid-physical-state');
 return snapshot.state==='terminal'&&physicalKeys.every(k=>snapshot.physical[k]===0);
}
export function verifySample(snapshot,expected){
 requireValue(physicallyEnded(snapshot),'not-ended');
 requireValue(snapshot.current===true&&snapshot.result?.reason==='completed'&&snapshot.result.delivery==='complete','unusable-result');
 requireValue(Array.isArray(snapshot.result.causes)&&snapshot.result.causes.length===0,'failed-result');
 requireValue(isDeepStrictEqual(snapshot.result.targets,[{node:'result',port:'out'}]),'wrong-target');
 const ports=snapshot.result.ports?.filter(p=>p.endpoint?.node==='result'&&p.endpoint?.port==='out');
 requireValue(ports?.length===1&&isDeepStrictEqual(ports[0].value,{status:'data',packet:{version:1,root:{type:'text',text:expected}}}),'wrong-output');
}
export function traceConfiguration(mode=0){
 requireValue(mode===0||mode===1||mode===2,'trace-mode');
 return {mode:['off','basic','host-detailed'][mode],operation:mode===0?null:{method:mode===1?'traceStart':'traceStartDetailed'}};
}
// Adjacent runs use the same case; reverse the order for each pair and group.
export function pairedSchedule(groups,samples){
 requireValue(Number.isSafeInteger(groups)&&groups>=1&&groups<=3&&Number.isSafeInteger(samples)&&samples>=2&&samples<=10000&&samples%2===0,'paired-count');
 return Array.from({length:groups},(_,g)=>Array.from({length:samples},(_,i)=>({group:g+1,pair:i+1,caseIndex:i%cases.length,modes:(g+i+Math.floor(i/cases.length))%2?[1,0]:[0,1]}))).flat();
}
export function benchmarkOptions(argv){
 requireValue(argv.length>0&&!argv[0].startsWith('--'),'usage');
 const result={folder:argv[0],samples:100,warmups:8,pollMs:5,trace:0,progressEvery:0};
 const names={'--samples':'samples','--warmups':'warmups','--poll-ms':'pollMs','--trace':'trace','--progress-every':'progressEvery'},seen=new Set();
 for(let i=1;i<argv.length;i+=2){const key=names[argv[i]];requireValue(key&&!seen.has(key)&&/^\d+$/.test(argv[i+1]??''),'usage');seen.add(key);result[key]=Number(argv[i+1]);}
 requireValue(Number.isSafeInteger(result.samples)&&result.samples>=1&&result.samples<=10000,'sample-count');
 requireValue(Number.isSafeInteger(result.warmups)&&result.warmups>=0&&result.warmups<=1000,'warmup-count');
 requireValue(Number.isSafeInteger(result.pollMs)&&result.pollMs>=1&&result.pollMs<=1000,'poll-interval');
 requireValue(Number.isSafeInteger(result.progressEvery)&&result.progressEvery>=0&&result.progressEvery<=10000,'progress-interval');traceConfiguration(result.trace);return result;
}
function verifyBundle(folder){
 const base=realpathSync(folder),raw=readFileSync(join(base,'bundle-manifest.json')),manifest=JSON.parse(raw.toString('utf8').replace(/^\uFEFF/,''));
 requireValue(manifest.formatVersion===1&&Array.isArray(manifest.files)&&manifest.files.length>0,'bundle-manifest');
 requireValue(manifest.nodeVersion==='24.22.1'&&manifest.wasmtimeVersion==='50.0.1','runtime-version');
 const files=new Map();
 for(const file of manifest.files){
  requireValue(typeof file.path==='string'&&!isAbsolute(file.path)&&!/[\\:]/.test(file.path)&&file.path.split('/').every(p=>p&&p!=='.'&&p!=='..')&&!files.has(file.path),'bundle-path');
  const full=realpathSync(join(base,file.path)),rel=relative(base,full);requireValue(rel&&!isAbsolute(rel)&&rel!=='..'&&!rel.startsWith('..'+(platform()==='win32'?'\\':'/')),'bundle-path');
  const bytes=readFileSync(full);requireValue(bytes.length===file.bytes&&hash(bytes)===file.sha256,'bundle-integrity');files.set(file.path,file);
 }
 for(const name of ['DIY_IME2.exe','runtime/node.exe','runtime/wasmtime.dll','runtime/server.mjs','bin/diy-ime2-external-host.exe','bin/diy-ime2-wasm-worker.exe','bin/diy-ime2-node-manager.exe','defaults/dictionary.tsv','examples/graphs/official-dictionary.json'])requireValue(files.has(name),'bundle-incomplete');
 requireValue(![...files.keys()].some(p=>p.includes('test-host')),'synthetic-host');
 const graphBytes=readFileSync(join(sourceRoot,'examples/graphs/official-dictionary.json')),graph=JSON.parse(graphBytes);
 requireValue(isDeepStrictEqual(JSON.parse(readFileSync(join(base,'examples/graphs/official-dictionary.json'))),graph),'graph-mismatch');
 requireValue(hash(readFileSync(join(sourceRoot,'distribution/defaults/dictionary.tsv')))===files.get('defaults/dictionary.tsv').sha256,'dictionary-mismatch');
 const wasm=[...files.values()].filter(f=>f.path.endsWith('.wasm'));requireValue(wasm.length===1,'module-layout');
 return {base,graph,provenance:{bundleManifestSha256:hash(raw),sourceCommit:typeof manifest.sourceCommit==='string'&&/^[a-f0-9]{40}$/.test(manifest.sourceCommit)?manifest.sourceCommit:null,sourceKind:['pinned-pipeline','development-worktree'].includes(manifest.sourceKind)?manifest.sourceKind:'unknown',nodeVersion:manifest.nodeVersion,wasmtimeVersion:manifest.wasmtimeVersion,moduleSha256:wasm[0].sha256,graphSha256:hash(graphBytes),dictionarySha256:files.get('defaults/dictionary.tsv').sha256,workerSha256:files.get('bin/diy-ime2-wasm-worker.exe').sha256,runtimeSha256:files.get('runtime/wasmtime.dll').sha256}};
}
export async function benchmark(config){
 let trace=traceConfiguration(config.trace);
 const schedule=config.pairedGroups===undefined?null:pairedSchedule(config.pairedGroups,config.samples);
 const progressEvery=config.progressEvery??0;
 requireValue(Number.isSafeInteger(progressEvery)&&progressEvery>=0&&progressEvery<=10000,'progress-interval');
 requireValue(platform()==='win32'&&arch()==='x64','windows-x64-required');
 const bundle=verifyBundle(config.folder),tempParent=realpathSync(tmpdir()),root=mkdtempSync(join(tempParent,'diyime-latency-')),data=join(root,'data');
 let child,closed,url,beat,launcherEnded=true;
 async function bounded(promise,ms,code){let timer;try{return await Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new BenchmarkError(code)),ms);})]);}finally{clearTimeout(timer);}}
 async function request(path,operation={},method='POST'){
  const response=await fetch(url+path,{method,signal:AbortSignal.timeout(120000),headers:{'Content-Type':'application/json',Origin:url.slice(0,-1)},body:method==='GET'||method==='DELETE'?undefined:JSON.stringify(operation)});
  requireValue(response.ok,'http-response');return await response.json();
 }
 async function call(path,operation,method){const value=await request(path,operation,method);requireValue(value.ok===true,'host-response');return value.data;}
 async function start(){
  const begin=performance.now();launcherEnded=false;
  child=spawn(join(bundle.base,'DIY_IME2.exe'),['--no-browser','--data-root',data,'--port','0'],{stdio:'pipe',windowsHide:true,env:{...process.env,PATH:''}});
  child.stderr.on('data',()=>{});closed=new Promise(resolve=>child.once('close',code=>{launcherEnded=true;resolve(code);}));
  const lines=createInterface({input:child.stdout});url=await bounded(new Promise((resolve,reject)=>{lines.once('line',line=>{try{const value=new URL(JSON.parse(line).url);requireValue(value.protocol==='http:'&&value.hostname==='127.0.0.1'&&Number(value.port)>0&&value.pathname==='/'&&!value.username&&!value.password&&!value.search&&!value.hash,'launcher-url');resolve(value.href);}catch{reject(new BenchmarkError('launcher-url'));}});child.once('error',()=>reject(new BenchmarkError('launcher-start')));child.once('close',()=>reject(new BenchmarkError('launcher-exited')));}),30000,'launcher-ready-timeout');
  let pending=false;beat=setInterval(()=>{if(pending)return;pending=true;void request('api/app/heartbeat').catch(()=>{}).finally(()=>{pending=false;});},1000);
  const deadline=performance.now()+240000;
  while(true){const state=await request('api/app',undefined,'GET');requireValue(state.phase!=='failed','setup-failed');if(state.phase==='ready'){requireValue(state.prepared===48,'official-package-count');return performance.now()-begin;}requireValue(performance.now()<deadline,'setup-timeout');await delay(30);}
 }
 async function stop(){
  const value=await request('api/app/close');requireValue(value.closed===true,'close-response');
  requireValue(await bounded(closed,120000,'close-timeout')===0,'launcher-exit');clearInterval(beat);child=undefined;
 }
 try{
  const initialReadyMs=await start(),setupBegin=performance.now();
  const connection=(await call('api/external/connections')).connectionId;
  requireValue(typeof connection==='string'&&/^[a-f0-9]{32}$/.test(connection),'connection-id');
  const host=operation=>call('api/external/connections/'+connection+'/request',operation);
  const definitions=(await host({method:'definitions'})).definitions;
  const inventory=JSON.parse(readFileSync(join(sourceRoot,'docs/node-migration-inventory.json'))).nodes.filter(n=>n.registered).map(n=>n.definition);
  requireValue(definitions.length===52&&definitions.every(d=>isDeepStrictEqual(d,inventory.find(e=>e.kind===d.kind))),'definition-mismatch');
  const graph=await call('api/examples/official-dictionary',undefined,'GET');requireValue(isDeepStrictEqual(graph,bundle.graph),'graph-mismatch');
  const prepareBegin=performance.now(),review=await host({method:'prepare',graph}),prepareMs=performance.now()-prepareBegin;
  requireValue(review.requests.length===9&&review.missing.length===9,'fresh-consent-required');
  const approveBegin=performance.now();await host({method:'approve',reviewId:review.reviewId,ids:review.missing.map(r=>r.id),save:false});const approveMs=performance.now()-approveBegin;
  const activateBegin=performance.now(),activated=await host({method:'activate',reviewId:review.reviewId}),activateMs=performance.now()-activateBegin;
  const plan=activated.state.activePlanVersion;requireValue(typeof plan==='string','active-plan');
  const setupMs=performance.now()-setupBegin;let revision=0,traceCursor='0',traceNextPoll=0;
  let profiler;
  const makeDiagnostics=()=>({enabled:trace.operation!==null,mode:trace.mode,pollIntervalMs:150,pages:0,events:0,hostIntervals:0,dropped:0,recordings:0,recordingBoundary:'per-run-outside-sample',browserRenderingIncluded:false,finalDrainOutsideSamples:true,phaseSummary:new PhaseProfiler(trace.mode).summary});
  let diagnostics=makeDiagnostics();
  const modes=schedule?[0,1].map(mode=>{trace=traceConfiguration(mode);return makeDiagnostics();}):null;
  function selectMode(mode){trace=traceConfiguration(mode);diagnostics=modes[mode];}
  async function pollTrace(force=false){
   if(!trace.operation||!force&&performance.now()<traceNextPoll)return true;
   const page=profiler.observe((await host({method:'traceRead',after:traceCursor,limit:256})).trace),previous=traceCursor;
   requireValue(page.events.every(e=>BigInt(e.sequence)>BigInt(previous)),'trace-cursor-stalled');
   diagnostics.pages++;diagnostics.events+=page.events.length;diagnostics.hostIntervals+=page.events.filter(e=>/^(host-request|worker-staging|worker-process|worker-cleanup|broker-exchange)-started$/.test(e.event)).length;
   traceCursor=page.events.at(-1)?.sequence??traceCursor;traceNextPoll=performance.now()+150;
   const caught=BigInt(traceCursor)>=BigInt(page.nextSequence)||page.events.length===0;
   requireValue(page.events.length>0||BigInt(traceCursor)>=BigInt(page.nextSequence)||page.dropped>0,'trace-coverage-missing');
   requireValue(caught||BigInt(traceCursor)>BigInt(previous),'trace-cursor-stalled');return caught;
  }

  async function run(test){
   // Reset only after the preceding run physically ended and all pages were drained.
   // This avoids age/capacity eviction during long comparisons without widening retention.
   if(trace.operation){
    profiler=new PhaseProfiler(trace.mode);traceCursor='0';traceNextPoll=0;
    const started=profiler.observe((await host(trace.operation)).trace);
    requireValue(started.recording&&started.recordingId!==null,'trace-start-not-recording');diagnostics.recordings++;
   }
   const begin=performance.now();revision++;
   if(revision>1)await host({method:'invalidateInput',expectedPlanVersion:plan,revision});
   const {receipt}=await host({method:'run',request:{startKey:String(revision),expectedPlanVersion:plan,targets:[{node:'result',port:'out'}],input:{revision,text:test.text,mode:test.mode}}});
   const deadline=begin+240000;
   while(true){const {snapshot}=await host({method:'getRun',key:receipt.key});if(physicallyEnded(snapshot)){
    const elapsedMs=performance.now()-begin;verifySample(snapshot,test.expected);
    // Stop/read/aggregate outside the measured boundary, separately for each recording.
    if(trace.operation){
     const stopped=decodeTracePage((await host({method:'traceStop'})).trace);
     requireValue(stopped.recordingId!==null,'trace-recording-changed');
     while(!await pollTrace(true)){};
     diagnostics.dropped+=stopped.dropped;
     mergePhaseSummaries(diagnostics.phaseSummary,profiler.finish());
    }
    return {caseId:test.id,elapsedMs};
   }requireValue(performance.now()<deadline,'run-timeout');await pollTrace();await delay(config.pollMs);}
  }
  let firstRun;const warmups=[],samples=[],pairs=[];
  if(schedule){
   firstRun=[];for(const mode of [0,1]){selectMode(mode);firstRun.push({mode:trace.mode,...await run(cases[0])});}
   for(let i=0;i<config.warmups;i++)for(const mode of (i+Math.floor(i/cases.length))%2?[1,0]:[0,1]){selectMode(mode);warmups.push({mode:trace.mode,...await run(cases[i%cases.length])});}
   for(const entry of schedule){
    const pair={group:entry.group,pair:entry.pair,runs:[]};
    for(const mode of entry.modes){selectMode(mode);const sample={group:entry.group,pair:entry.pair,mode:trace.mode,...await run(cases[entry.caseIndex])};samples.push(sample);pair.runs.push(sample);}
    pairs.push(pair);
    // Persist only completed pairs, after both measured runs and diagnostic drains.
    if(config.onPair)await config.onPair(pair);
    if(progressEvery&&entry.pair%progressEvery===0)console.error(`Completed group ${entry.group}/${config.pairedGroups}: ${entry.pair}/${config.samples} pairs`);
   }
  }else{
   firstRun=await run(cases[0]);for(let i=0;i<config.warmups;i++)warmups.push(await run(cases[i%cases.length]));
   for(let i=0;i<config.samples;i++){
    samples.push(await run(cases[i%cases.length]));
    if(progressEvery&&samples.length%progressEvery===0)console.error(`Completed ${samples.length}/${config.samples} samples (${trace.mode})`);
   }
  }
  if(trace.mode==='host-detailed')requireValue(diagnostics.hostIntervals>0||diagnostics.dropped>0,'host-detail-missing');
  await call('api/external/connections/'+connection,undefined,'DELETE');await stop();
  const existingCatalogReadyMs=await start();await stop();
  if(schedule){
   const groups=Array.from({length:config.pairedGroups},(_,i)=>{
    const summaries=Object.fromEntries(['off','basic'].map(mode=>[mode,summarize(samples.filter(s=>s.group===i+1&&s.mode===mode).map(s=>s.elapsedMs))]));
    return {group:i+1,summaries,p95IncreasePercent:(summaries.basic.p95Ms/summaries.off.p95Ms-1)*100};
   });
   return {formatVersion:1,kind:'bundled-wasm-paired-trace',scope:'small bundled test-dictionary graph; not Reference IME or TSF',provenance:bundle.provenance,environment:{platform:platform(),arch:arch(),osRelease:release(),driverNode:process.versions.node},measurement:{clock:'Node performance.now milliseconds',boundary:'before input invalidation/run admission through observed terminal and all four physical counts zero; includes loopback JSON and polling',pollIntervalMs:config.pollMs,diagnosticPollMs:150,quantile:'nearest rank: sorted[ceil(n*p)-1]',firstRunExcluded:true,warmupsExcluded:true,caseOrder:'same case per adjacent pair; order counterbalanced by group and occurrence of each case',orderVersion:'case-occurrence-v2',recordingBoundary:'per-run-outside-sample',browserRenderingIncluded:false,finalDrainOutsideSamples:true},lifecycle:{initialReadyMs,setupMs,prepareMs,approveMs,activateMs,existingCatalogReadyMs,firstRun},warmups,pairs,groups,diagnostics:modes,checks:{bundleHashes:true,definitions:52,officialPackages:48,exactGraph:true,explicitSessionConsent:true,allOutputsVerified:true,allPhysicalEnded:true,trueClose:true,noToolchainPath:true}};
  }
  const byCase=Object.fromEntries(cases.map(c=>[c.id,samples.filter(s=>s.caseId===c.id).map(s=>s.elapsedMs)]).filter(([,v])=>v.length).map(([id,v])=>[id,summarize(v)]));
  return {formatVersion:1,diagnostics,kind:'bundled-wasm-latency',scope:'small bundled test-dictionary graph; not Reference IME Latency or TSF',provenance:bundle.provenance,environment:{platform:platform(),arch:arch(),osRelease:release(),driverNode:process.versions.node},measurement:{clock:'Node performance.now milliseconds',boundary:'before input invalidation/run admission through observed terminal and all four physical counts zero; includes loopback JSON and polling',pollIntervalMs:config.pollMs,quantile:'nearest rank: sorted[ceil(n*p)-1]',warmupsExcluded:true,firstRunExcluded:true,caseOrder:'fixed round robin',notSteadyStateGuest:'every invocation retains fresh isolated Worker lifecycle'},lifecycle:{initialReadyMs,setupMs,prepareMs,approveMs,activateMs,existingCatalogReadyMs,firstRun},warmups,samples,summary:summarize(samples.map(s=>s.elapsedMs)),byCase,checks:{bundleHashes:true,definitions:52,officialPackages:48,exactGraph:true,explicitSessionConsent:true,allOutputsVerified:true,allPhysicalEnded:true,trueClose:true,noToolchainPath:true}};
 }finally{
  clearInterval(beat);
  if(child&&!launcherEnded){try{await stop();}catch{child.kill();await bounded(closed,10000,'forced-close-timeout');}}
  requireValue(launcherEnded,'cleanup-live-launcher');
  const expectedParent=tempParent;requireValue(dirname(root)===expectedParent&&root.startsWith(join(expectedParent,'diyime-latency-'))&&!lstatSync(root).isSymbolicLink(),'unsafe-cleanup');rmSync(root,{recursive:true});
 }
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 if(process.argv[2]==='--help')console.log('node scripts/benchmark-windows-application.mjs BUILT_FOLDER [--samples 100] [--warmups 8] [--poll-ms 5] [--trace 0|1|2] [--progress-every 0] (0=off, 1=basic, 2=Host detailed; progress writes counts to stderr outside samples)');
 else{try{console.log(JSON.stringify(await benchmark(benchmarkOptions(process.argv.slice(2)))));}catch(error){console.error('Benchmark failed: '+(error instanceof BenchmarkError?error.code:'unexpected')+'; no successful report emitted');process.exitCode=1;}}
}
