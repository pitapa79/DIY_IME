import {decodeTracePage} from '../editor/src/executionTrace.ts';
import {spawn,spawnSync} from 'node:child_process';
import {mkdtempSync,readFileSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';import {join,resolve,dirname} from 'node:path';import {createHash} from 'node:crypto';import {createInterface} from 'node:readline';import {setTimeout as delay} from 'node:timers/promises';import {createServer} from 'node:http';import assert from 'node:assert/strict';
const [exe,producer,worker,dll,wasm,policyFile]=process.argv.slice(2);if(!policyFile)throw Error('usage: HOST PRODUCER WORKER DLL WASM RESOURCE_CONTRACT');
const root=mkdtempSync(join(tmpdir(),'diyime-network-host-')),records=[],held=[];let hold=false,replyBody='remote-response',replyStatus=200,extraHeaders={};
const server=createServer(async(req,res)=>{const chunks=[];for await(const chunk of req)chunks.push(chunk);records.push({url:req.url,method:req.method,headers:req.headers,body:Buffer.concat(chunks)});const bytes=Buffer.from(replyBody);res.writeHead(replyStatus,{'Content-Length':bytes.length,...extraHeaders});res.flushHeaders();if(hold){hold=false;held.push(()=>res.end(bytes));}else res.end(bytes);});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;
const module=readFileSync(wasm),policy=JSON.parse(readFileSync(policyFile,'utf8')).resourceProfiles.bounded;Object.assign(policy,{retentionMs:60000,maxPhysicalJobs:1,maxQueuedJobs:0});
const config={packageManager:{dataRoot:join(root,'data'),workspace:join(root,'workers'),worker,runtimeDll:dll,policy:{processMemoryBytes:536870912,wasmMemoryBytes:134217728,fuelPerCall:null,walltimeMs:null,terminationGraceMs:5}},permissionRoot:join(root,'permissions'),secretRoot:join(root,'secrets'),resourcePolicy:policy,requestBytes:1048576,responseBytes:16777216,network:{bindings:{api:{host:'127.0.0.1',port,secure:false,allowInsecureLoopback:true,method:'POST',path:'/fixed?selected=1',contentType:'application/octet-stream',requestBytes:4096,responseBytes:4096,headerBytes:4096,timeouts:{resolveMs:3000,connectMs:3000,sendMs:3000,receiveMs:10000,budgetMs:15000},authentication:{secret:'service',header:'Authorization',prefix:'Bearer '}}},limits:{maxCalls:2,maxRequestBytes:6000,maxResponseBytes:6000,maxContinuationBytes:1024,budgetMs:60000}}};
const configPath=join(root,'host.json');function configurations(){writeFileSync(configPath,JSON.stringify(config));}configurations();
function command(args,success=true){const r=spawnSync(producer,args,{windowsHide:true,encoding:'utf8'});if(success)assert.equal(r.status,0,r.stderr);else assert.notEqual(r.status,0);return r;}
class Client{
 constructor(){this.child=spawn(exe,[configPath],{stdio:'pipe',windowsHide:true});this.errors=[];this.child.stderr.on('data',b=>this.errors.push(b));this.pending=new Map();this.serial=0;this.ended=new Promise(resolve=>this.child.on('close',code=>{for(const p of this.pending.values())p.reject(Error('Host ended '+code+': '+Buffer.concat(this.errors)));this.pending.clear();resolve(code);}));createInterface({input:this.child.stdout}).on('line',line=>{const reply=JSON.parse(line),p=this.pending.get(reply.id);if(p){this.pending.delete(reply.id);p.resolve(reply);}});}
 send(operation){const id=String(++this.serial),json=Buffer.from(JSON.stringify({id,operation})),header=Buffer.alloc(12);header.writeUInt32LE(json.length);header.writeBigUInt64LE(0n,4);const promise=new Promise((resolve,reject)=>this.pending.set(id,{resolve,reject}));promise.id=id;this.child.stdin.write(Buffer.concat([header,json]));return promise;}
 async call(operation){const r=await this.send(operation);assert(r.ok,JSON.stringify(r.error));return r.data;}
 async close(){this.child.stdin.end();assert.equal(await this.ended,0,Buffer.concat(this.errors).toString());}
}
async function until(check){const end=Date.now()+60000;for(;;){const value=await check();if(value)return value;if(Date.now()>end)throw Error('network Host did not settle');await delay(20);}}
const text='private-network-input',secret='synthetic-network-private-api-key';let host,sequence=0,revision=1;
const graph=(mode='network',source='source')=>({version:1,nodes:[{id:'source',kind:'input.text',version:1},{id:'other-source',kind:'input.text',version:1},{id:'node',kind:'fixture.wasm',version:1,config:{mode}}],edges:[{from:{node:source,port:'out'},to:{node:'node',port:'in'}}]});
async function prepare(mode='network',source='source'){return host.call({method:'prepare',graph:graph(mode,source)});}
async function approve(review,save=true,select=()=>true){if(review.missing.filter(select).length)await host.call({method:'approve',reviewId:review.reviewId,ids:review.missing.filter(select).map(r=>r.id),save});}
async function activate(mode='network',source='source'){const review=await prepare(mode,source);await approve(review);assert.equal((await host.call({method:'activate',reviewId:review.reviewId})).phase,'ready');sequence=0;revision=1;return review;}
async function start(){return (await host.call({method:'run',request:{startKey:String(++sequence),expectedPlanVersion:'1',targets:[{node:'node',port:'out'}],input:{revision,text,mode:'compose'}}})).receipt.key;}
async function terminal(key){return until(async()=>{const {snapshot}=await host.call({method:'getRun',key});return snapshot.state==='terminal'&&Object.values(snapshot.physical).every(n=>n===0)?snapshot:null;});}
function value(snapshot){return snapshot.result.ports.find(p=>p.endpoint.node==='node').value;}
async function run(){return terminal(await start());}
async function unchangedRequests(action){const count=records.length;await action();assert.equal(records.length,count);}
function release(){while(held.length)held.shift()();}
try{
 const definition={kind:'fixture.wasm',version:1,inputs:[{port:{id:'in',dataType:{id:'text-packet',version:1},language:'ja-Latn',root:'text'},required:true}],outputs:[{id:'out',dataType:{id:'text-packet',version:1},language:null,root:'text'}],configSchema:{type:'object'},defaultConfig:{mode:'echo'}};
 const manifest=join(root,'manifest.json'),packed=join(root,'fixture.diynode');writeFileSync(manifest,JSON.stringify({packageId:'test.network',version:'1.0.0',displayName:'Network fixture',runtime:'wasm32',guestApiVersion:1,node:definition,moduleSha256:createHash('sha256').update(module).digest('hex'),capabilities:[{id:'network.request',binding:'api'},{id:'secret.use',binding:'api'}]}));rmSync(packed,{force:true});command(['pack',manifest,wasm,packed]);command(['import',config.packageManager.dataRoot,join(root,'install'),worker,dll,packed,'--commit']);
 host=new Client();await host.call({method:'secretPut',id:'service',value:secret});
 const first=await prepare();assert.deepEqual(first.requests.map(r=>r.kind).sort(),['execute','inputFlow','networkUse','secretUse']);assert.equal(records.length,0);const network=first.requests.find(r=>r.kind==='networkUse').network;assert.equal(network.destination,'http://127.0.0.1:'+port+'/fixed?selected=1');assert.equal(network.secret.id,'service');for(const privateValue of [secret,config.secretRoot,config.packageManager.workspace,'ciphertext','Authorization'])assert(!JSON.stringify(first).includes(privateValue));
 await approve(first,true,r=>!['networkUse','secretUse'].includes(r.kind));assert.equal((await host.send({method:'activate',reviewId:first.reviewId})).error.code,'permission-required');assert.equal(records.length,0);
 const second=await prepare();assert.deepEqual(second.missing.map(r=>r.kind).sort(),['networkUse','secretUse']);await approve(second,true,r=>r.kind==='networkUse');assert.equal((await host.send({method:'activate',reviewId:second.reviewId})).error.code,'permission-required');assert.equal(records.length,0);
 const approved=await activate();assert.deepEqual(approved.missing.map(r=>r.kind),['secretUse']);
 const normal=await run();assert.equal(value(normal).status,'data',JSON.stringify(normal));const resumed=JSON.parse(value(normal).packet.root.text);assert.equal(resumed.operation.method,'resume');assert.equal(Buffer.from(resumed.operation.response.body).toString(),replyBody);assert.equal(resumed.operation.continuation.text,text);for(const field of ['inputs','mode','resources','runKey','input','secret','headers'])assert.equal(resumed.operation[field],undefined);assert(!JSON.stringify(resumed).includes(secret));assert.equal(records[0].url,'/fixed?selected=1');assert.equal(records[0].method,'POST');assert.equal(records[0].body.toString(),text);assert.equal(records[0].headers.authorization,'Bearer '+secret);console.log('Network normal/partial consent/projection passed');
 await activate('networkTwice');await host.call({method:'traceStartDetailed'});const before=records.length;assert.equal(value(await run()).status,'data');assert.equal(records.length,before+2);assert.equal(records.at(-1).body.toString(),replyBody);
 const detail=decodeTracePage((await host.call({method:'traceRead',after:'0',limit:256})).trace);
 for(const [phase,count] of [['broker-exchange',2],['worker-process',3]]){
  assert.equal(detail.events.filter(e=>e.event===phase+'-started').length===count||detail.dropped>0,true);
  assert.equal(detail.events.filter(e=>e.event===phase+'-finished').length===count||detail.dropped>0,true);
 }
 for(const privateValue of [text,secret,config.packageManager.workspace,'Authorization','/fixed?selected=1'])assert(!JSON.stringify(detail).includes(privateValue));
 console.log('Detailed broker/continuation intervals passed');
 if(detail.dropped===0){
  const intervals=detail.events.flatMap(e=>e.worker?[e.worker]:[]);
  assert.deepEqual([...new Set(intervals.map(w=>w.instance))],[1,2,3]);
  for(const instance of [1,2,3])for(const phase of ['compile','instantiate','guest-call'])assert(intervals.some(w=>w.instance===instance&&w.phase===phase&&w.finishedUs!==undefined));
 }
 for(const [mode,code] of [['networkUnknown','package-capability'],['networkOversizeBody','network-limit'],['networkHugeContinuation','network-limit'],['networkBadBody','worker-reply'],['networkNoContinuation','worker-reply'],['networkMixed','worker-reply'],['networkDuplicate','worker-reply']]){await activate(mode);await unchangedRequests(async()=>{const snapshot=await run();assert.notEqual(value(snapshot).status,'data',mode);assert(JSON.stringify(snapshot).includes(code),JSON.stringify(snapshot));});}
 await activate('networkMany');const manyBefore=records.length;const many=await run();assert.notEqual(value(many).status,'data');assert.equal(records.length,manyBefore+2);assert(JSON.stringify(many).includes('network-limit'));console.log('Network strict wire/hops/bounds passed');
 for(const operation of ['cancel','input','protect','revoke','rotate','eof']){
  console.log('Network held IO starting '+operation);const review=await activate();hold=true;const previous=records.length,key=await start();await until(()=>held.length===1);const active=await host.call({method:'status'});assert.equal(active.workerProcesses,0);assert.equal(active.physical.physicalJobs,1);assert.equal(records.length,previous+1);let pending;
  if(operation==='cancel')await host.call({method:'cancelRun',key});
  if(operation==='input'){revision=2;await host.call({method:'invalidateInput',expectedPlanVersion:'1',revision});}
  if(operation==='protect')await host.call({method:'protection',enabled:true});
  if(operation==='revoke')await host.call({method:'revoke',id:review.requests.find(r=>r.kind==='secretUse').id});
  if(operation==='rotate'){pending=host.send({method:'secretPut',id:'service',value:secret+'-new'});}
  if(operation==='eof'){host.child.stdin.end();pending=host.ended;}
  let done=false;pending?.then(()=>{done=true;});await delay(120);
  if(pending)assert.equal(done,false,'physical IO must retain mutation/close');else {const status=await host.call({method:'status'});assert.equal(status.physical.physicalJobs,1);assert.equal(status.workerProcesses,0);}
  release();
  if(operation==='rotate'){assert((await pending).ok);const changed=await prepare();assert(changed.missing.some(r=>r.kind==='secretUse'));await host.call({method:'secretPut',id:'service',value:secret});}
  else if(operation==='eof'){assert.equal(await pending,0);host=undefined;}
  else {await until(async()=>{const status=await host.call({method:'status'});return status.physical.physicalJobs===0&&status.workerProcesses===0;});if(['cancel','input'].includes(operation)){const snapshot=(await host.call({method:'getRun',key})).snapshot;if(operation==='input'){assert.equal(snapshot.current,false);assert.equal(snapshot.result,null);}else{assert.equal(snapshot.result.reason,'cancelled');assert.notEqual(value(snapshot).status,'data');assert(!JSON.stringify(snapshot).includes(replyBody));}}else assert.equal((await host.send({method:'getRun',key})).ok,false);}
  assert.equal(records.length,previous+1,'cancelled response must not resume/request again');
  if(operation==='protect')await host.call({method:'protection',enabled:false});if(!host)host=new Client();
  console.log('Network held IO '+operation+' passed');
 }
 await activate();assert.equal(value(await run()).status,'data');
 const rerouted=await prepare('network','other-source');assert.deepEqual(rerouted.missing.map(r=>r.kind),['inputFlow']);
 const stored=readFileSync(join(root,'permissions/permissions.json'),'utf8');for(const privateValue of [secret,text,'127.0.0.1','service','api',config.secretRoot])assert(!stored.includes(privateValue));
 await host.close();host=undefined;
 // A stored installation is not a runtime proof when providers or vault are absent.
 const networkConfig=config.network;
 delete config.network;configurations();host=new Client();await unchangedRequests(async()=>assert.equal((await host.send({method:'prepare',graph:graph()})).error.code,'package-capability'));await host.close();host=undefined;
 config.network=networkConfig;const secretRoot=config.secretRoot;delete config.secretRoot;configurations();host=new Client();await unchangedRequests(async()=>assert.equal((await host.send({method:'prepare',graph:graph()})).error.code,'secret-unavailable'));await host.close();host=undefined;config.secretRoot=secretRoot;
 // Aggregate request refusal happens before the second dispatch; response caps
 // also apply during a later hop, rather than only after its full body is read.
 const limits={...config.network.limits};
 for(const kind of ['request','response']){
  config.network.limits={...limits,[kind==='request'?'maxRequestBytes':'maxResponseBytes']:kind==='request'?Buffer.byteLength(text)+Buffer.byteLength(replyBody)-1:Buffer.byteLength(replyBody)+1};configurations();host=new Client();await activate('networkTwice');const count=records.length;const snapshot=await run();assert.notEqual(value(snapshot).status,'data');assert(JSON.stringify(snapshot).includes('network-limit'));assert.equal(records.length,count+(kind==='request'?1:2));await host.close();host=undefined;
 }
 config.network.limits=limits;configurations();host=new Client();await activate();replyBody=secret;const echo=await run();assert.notEqual(value(echo).status,'data');assert(JSON.stringify(echo).includes('network-secret'));assert(!JSON.stringify(echo).includes(secret));replyBody='remote-response';
 replyStatus=302;extraHeaders={Location:'http://127.0.0.1:'+port+'/redirected'};const redirectCount=records.length;const redirect=await run();assert.equal(JSON.parse(value(redirect).packet.root.text).operation.response.status,302);assert.equal(records.length,redirectCount+1);replyStatus=200;extraHeaders={};await host.close();host=undefined;
 // Session-only network/secret grants expire on a new graph preparation.
 config.permissionRoot=join(root,'session-permissions');configurations();host=new Client();const temporary=await prepare();await approve(temporary,false);await host.call({method:'activate',reviewId:temporary.reviewId});sequence=0;assert.equal(value(await run()).status,'data');const next=await prepare();assert.equal(next.missing.length,next.requests.length);await host.close();host=undefined;
 // Package declarations cannot claim extra secret use, omit required secret
 // use, or obtain network authority by merely emitting a call.
 const authentication=config.network.bindings.api.authentication;
 for(const [name,capabilities,auth,expected] of [
  ['missing-secret',[{id:'network.request',binding:'api'}],authentication,'package-capability'],
  ['extra-secret',[{id:'network.request',binding:'api'},{id:'secret.use',binding:'api'}],undefined,'package-capability'],
  ['plain',[{id:'network.request',binding:'api'}],undefined,null],
  ['undeclared',[],undefined,'runtime-denied'],
 ]){
  console.log('Network capability starting '+name);config.packageManager.dataRoot=join(root,'data-'+name);config.permissionRoot=join(root,'permissions-'+name);config.network.bindings.api.authentication=auth;configurations();const m=JSON.parse(readFileSync(manifest,'utf8'));m.capabilities=capabilities;writeFileSync(manifest,JSON.stringify(m));rmSync(packed,{force:true});command(['pack',manifest,wasm,packed]);command(['import',config.packageManager.dataRoot,join(root,'install-'+name),worker,dll,packed,'--commit']);host=new Client();
  if(expected==='package-capability')await unchangedRequests(async()=>assert.equal((await host.send({method:'prepare',graph:graph()})).error.code,expected));
  else {const review=await activate();assert.equal(review.requests.some(r=>r.kind==='secretUse'),false);const count=records.length,snapshot=await run();if(expected){assert.notEqual(value(snapshot).status,'data');assert(JSON.stringify(snapshot).includes('package-capability'));assert.equal(records.length,count);}else{assert.equal(value(snapshot).status,'data');assert.equal(records.length,count+1);assert.equal(records.at(-1).headers.authorization,undefined);}}
  await host.close();host=undefined;
 }
 for(const capabilities of [[{id:'secret.use',binding:'api'}],[{id:'network.request',binding:'api'},{id:'secret.use',binding:'other'}],[{id:'network.unknown',binding:'api'}]]){const m=JSON.parse(readFileSync(manifest,'utf8'));m.capabilities=capabilities;writeFileSync(manifest,JSON.stringify(m));rmSync(packed,{force:true});command(['pack',manifest,wasm,packed]);command(['import',join(root,'unsupported-'+String(capabilities.length)),join(root,'unsupported-install'),worker,dll,packed,'--commit'],false);}
 console.log('Network providers/capabilities/aggregate limits/secret echo/redirect/Session expiry passed');
 console.log('Network broker: actual isolated continuation, separate consent, strict wire, bounded hops and true ending under cancel/input/protect/revoke/rotation/EOF passed.');
}finally{release();if(host)await host.close();await new Promise(resolve=>server.close(resolve));if(dirname(root)!==resolve(tmpdir())||!root.startsWith(join(tmpdir(),'diyime-network-host-')))throw Error('unexpected cleanup root');rmSync(root,{recursive:true});}
