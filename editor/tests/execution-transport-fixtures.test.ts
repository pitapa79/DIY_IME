import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge.ts';
const ROOT=resolve(fileURLToPath(new URL('..',import.meta.url)),'..');
const HOST=resolve(ROOT,'target/debug/examples',process.platform==='win32'?'execution_transport_fixtures.exe':'execution_transport_fixtures');
const fixture=JSON.parse(readFileSync(resolve(ROOT,'tests/contracts/execution_control.json'),'utf8'));
const IDS=['B03-lost-response-can-be-queried','B07-reconnect-rejects-old-run-key','B08-v2-does-not-negotiate-v3-implicitly'];
type RecordValue=Record<string,any>;
function selected(id:string) { const cases=fixture.cases.filter((c:RecordValue)=>c.id===id); expect(cases).toHaveLength(1); only(cases[0],['id','area','initial','events','expected']); expect(cases[0].area).toBe(id===IDS[2]?'compatibility':'transport'); return cases[0] as RecordValue; }
function only(value:RecordValue,allowed:string[]) { for(const key of Object.keys(value)) if(!allowed.includes(key)) throw new Error(`unsupported fixture field ${key}`); }
async function until(check:()=>boolean) { const end=Date.now()+5000; while(!check()){if(Date.now()>=end) throw new Error('fixture barrier timeout'); await new Promise(resolve=>setTimeout(resolve,2));} }
function jsonFile(path:string):any { try{return JSON.parse(readFileSync(path,'utf8'));}catch{return undefined;} }
function starts(dir:string):RecordValue[] { const path=join(dir,'starts.jsonl'); if(!existsSync(path))return[]; const text=readFileSync(path,'utf8'); const lines=text.split('\n'); lines.pop(); return lines.filter(Boolean).map(line=>JSON.parse(line)); }
function cleanup(dir:string) { const actual=realpathSync(dir),base=realpathSync(tmpdir()); if(dirname(actual)!==base||!basename(actual).startsWith('diy-ime-transport-formal-'))throw new Error('unexpected fixture directory'); rmSync(actual,{recursive:true,force:true}); }
function environment(c:RecordValue) {
  only(c.initial,['sessionEpoch','planVersion','latestInputRevision','resourceProfile','workload']);
  expect(c.initial.workload).toBe('single'); expect(c.initial.sessionEpoch).toMatch(/^[0-9a-f]{32}$/);
  expect(c.initial.latestInputRevision).toBeGreaterThan(0);
  const directory=mkdtempSync(join(tmpdir(),'diy-ime-transport-formal-'));
  const policyFile=join(directory,'policy.json'); const policy=fixture.resourceProfiles[c.initial.resourceProfile]; expect(policy).toBeDefined(); writeFileSync(policyFile,JSON.stringify(policy));
  return {directory,policyFile,policy,previous:process.env.DIY_IME_TRANSPORT_FIXTURE_DIR};
}
function graph(c:RecordValue,kind:string) { const work=fixture.workloads[c.initial.workload]; only(work,['jobs','policy','targets']); expect(work.policy).toBe('test-all-targets'); return {version:1,nodes:work.jobs.map((id:string)=>({id,kind,version:1})),edges:[]}; }
function targets(c:RecordValue) {return fixture.workloads[c.initial.workload].targets.map((target:string)=>{const parts=target.split('.'); expect(parts).toHaveLength(2); return {node:parts[0],port:parts[1]};});}
interface Metrics {acceptedRuns:number; sameRunKey?:boolean; automaticResubmissions?:number; physicalJobs?:number; errors:string[]; sessionEpoch?:string; previousResultKnown?:boolean; planRevisionUnchanged?:boolean}
function assertExpected(c:RecordValue,metrics:Metrics,epochs:Map<string,string>) {
  const actual:RecordValue={};
  for(const [field,want] of Object.entries(c.expected)) {
    switch(field) {
      case 'acceptedRuns': case 'sameRunKey': case 'automaticResubmissions': case 'physicalJobs': case 'errors': case 'previousResultKnown': case 'planRevisionUnchanged':
        expect(metrics[field as keyof Metrics],`${c.id}.${field}`).toEqual(want); actual[field]=metrics[field as keyof Metrics]; break;
      case 'sessionEpoch': {
        const bound=epochs.get(want as string); expect(bound,'expected epoch must be bound to a real issued epoch').toBeDefined(); expect(metrics.sessionEpoch).toBe(bound); actual.sessionEpoch=metrics.sessionEpoch; break;
      }
      default:throw new Error(`unsupported expected field ${field}`);
    }
  }
  expect(Object.keys(actual)).toHaveLength(Object.keys(c.expected).length);
}
async function clock(dir:string,at:number) {
  expect(Number.isSafeInteger(at)&&at>=0).toBe(true); writeFileSync(join(dir,'clock-request'),String(at));
  await until(()=> {const ack=jsonFile(join(dir,'clock-ack')); return ack?.requested===at&&ack?.observed===at;});
  expect(existsSync(join(dir,'clock-error'))).toBe(false);
}
async function httpCase(c:RecordValue) {
  const f=environment(c); process.env.DIY_IME_TRANSPORT_FIXTURE_DIR=f.directory;
  const middleware=createApiMiddleware({execution:{policyFile:f.policyFile,core:HOST,root:ROOT,responseTimeoutMs:5000}});
  const server=createServer((req,res)=>middleware(req,res,()=>{res.statusCode=404;res.end();})); await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));
  const address=server.address(); if(!address||typeof address==='string')throw new Error('address'); const origin=`http://127.0.0.1:${address.port}`;
  const post=async(path:string,request:unknown)=>fetch(origin+path,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(request)});
  const open=async()=>{const response=await post('/api/v3/sessions',{});expect(response.status).toBe(200);return (await response.json()).data;};
  const call=async(id:string,request:unknown)=>{const response=await post(`/api/v3/sessions/${id}/request`,request);return{status:response.status,body:await response.json()};};
  const epochs=new Map<string,string>(),runs=new Map<string,any>(); const metrics:Metrics={acceptedRuns:0,errors:[]};
  let session:any;
  try {
    session=await open(); epochs.set(c.initial.sessionEpoch,session.id); expect(session.resourcePolicy).toEqual(f.policy);
    const domain=session.resourceDomain;
    const version=Number(c.initial.planVersion); expect(Number.isSafeInteger(version)&&version>0).toBe(true);
    for(let n=0;n<version;n++){const prepared=await call(session.id,{method:'prepare',graph:graph(c,'test.fixture-held')});expect(prepared.status).toBe(200);const active=await call(session.id,{method:'activate',token:prepared.body.data.result.token});expect(active.body.data.state.activePlanVersion).toBe(String(n+1));}
    const invalidated=await call(session.id,{method:'invalidate-input',expectedPlanVersion:c.initial.planVersion,revision:c.initial.latestInputRevision}); expect(invalidated.status).toBe(200);
    for(const event of c.events) {
      const fields:Record<string,string[]>={start:['run','startKey','input','runBudgetMs','jobBudgets','dropReply'],'get-start':['startKey'],disconnect:[],reconnect:['sessionEpoch'],poll:['run','sessionEpoch']};
      if(!Object.hasOwn(fields,event.op))throw new Error(`unsupported formal op ${event.op}`);
      only(event,['atMs','op',...fields[event.op]]);
      await clock(f.directory,event.atMs);
      switch(event.op) {
        case 'start': {
          const response=await post(`/api/v3/sessions/${session.id}/request`,{method:'start-run',startKey:event.startKey,expectedPlanVersion:c.initial.planVersion,targets:targets(c),input:event.input,runBudgetMs:event.runBudgetMs,jobBudgets:event.jobBudgets});
          expect(response.status).toBe(200);metrics.acceptedRuns++;
          if(event.dropReply){await response.body?.cancel();} else {const receipt=(await response.json()).data.result; expect(receipt.state).toBe('accepted'); runs.set(event.run,receipt.key);}
          await until(()=>starts(f.directory).length===metrics.acceptedRuns*fixture.workloads[c.initial.workload].jobs.length);
          const observed=starts(f.directory).at(-1)!; expect(observed.nodeId).toBe(fixture.workloads[c.initial.workload].jobs[0]); expect(observed.input).toEqual(event.input); expect(observed.clockMs).toBe(event.atMs); expect(observed.key.sessionEpoch).toBe(session.id);expect(observed.key.planVersion).toBe(c.initial.planVersion);
          if(!event.dropReply)expect(runs.get(event.run)).toEqual(observed.key);
          break;
        }
        case 'get-start': {
          const query=await call(session.id,{method:'get-start',startKey:event.startKey}); expect(query.status).toBe(200); const key=query.body.data.result.key;
          metrics.sameRunKey=isDeepStrictEqual(key,starts(f.directory)[0].key);
          const snapshot=await call(session.id,{method:'get-run',key});expect(snapshot.status).toBe(200);
          expect(snapshot.body.data.result.elapsedMs).toBe(event.atMs); const physical=snapshot.body.data.result.physical; metrics.physicalJobs=physical.running+physical.cancelRequested+physical.stalled;
          break;
        }
        case 'disconnect': {const response=await fetch(`${origin}/api/v3/sessions/${session.id}`,{method:'DELETE'});expect(response.status).toBe(200);await until(()=>existsSync(join(f.directory,'cancelled')));break;}
        case 'reconnect': {const previous=session.id;session=await open();expect(session.id).not.toBe(previous);expect(session.resourceDomain).toBe(domain);expect(session.state.activePlanVersion).toBeNull();epochs.set(event.sessionEpoch,session.id);metrics.sessionEpoch=session.id;break;}
        case 'poll': {
          const key=runs.get(event.run);expect(key).toBeDefined();expect(key.sessionEpoch).toBe(epochs.get(event.sessionEpoch)); const reply=await call(session.id,{method:'get-run',key});
          if(reply.body.error)metrics.errors.push(reply.body.error.code);
          metrics.previousResultKnown=reply.status===200&&!!reply.body.data?.result?.result;
          expect(reply.status).toBe(422);expect(Object.hasOwn(reply.body,'data')).toBe(false);break;
        }
        default:throw new Error(`unsupported formal op ${event.op}`);
      }
    }
    metrics.automaticResubmissions=starts(f.directory).length-metrics.acceptedRuns*fixture.workloads[c.initial.workload].jobs.length;
    assertExpected(c,metrics,epochs);
  } finally {
    try {
      writeFileSync(join(f.directory,'release-producer'),'1'); middleware.close();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));await until(()=>!existsSync(f.policyFile+'.broker-lock'));
    } finally {
      if(f.previous===undefined)delete process.env.DIY_IME_TRANSPORT_FIXTURE_DIR;else process.env.DIY_IME_TRANSPORT_FIXTURE_DIR=f.previous;
    }
    cleanup(f.directory);
  }
}
async function formal_b03(){await httpCase(selected(IDS[0]));}
async function formal_b07(){await httpCase(selected(IDS[1]));}
async function formal_b08(){
  const c=selected(IDS[2]),f=environment(c); const child=spawn(HOST,['session'],{cwd:ROOT,windowsHide:true,stdio:'pipe',env:{...process.env,DIY_IME_TRANSPORT_FIXTURE_DIR:f.directory}});
  const exited=once(child,'close');const lines=createInterface({input:child.stdout}); const replies:any[]=[],waiters:((value:any)=>void)[]=[];
  lines.on('line',line=>{const value=JSON.parse(line);const waiter=waiters.shift();if(waiter)waiter(value);else replies.push(value);});
  let id=0;
  const call=async(request:unknown)=>{const expected=++id;child.stdin.write(JSON.stringify({protocol:2,id:expected,request})+'\n');let timer:ReturnType<typeof setTimeout>|undefined;const next=replies.length?Promise.resolve(replies.shift()):new Promise<any>((resolve,reject)=>{waiters.push(resolve);timer=setTimeout(()=>reject(new Error('v2 reply timeout')),5000);});try{const reply=await next;expect(reply.protocol).toBe(2);expect(reply.id).toBe(expected);return reply;}finally{if(timer)clearTimeout(timer);}};
  try {
    const version=Number(c.initial.planVersion);expect(Number.isSafeInteger(version)&&version>0).toBe(true);
    for(let n=0;n<version;n++){const prepared=await call({method:'prepare',graph:graph(c,'input.text')});expect(prepared.ok).toBe(true);const active=await call({method:'activate',token:prepared.state.prepared.token});expect(active.state.active.version).toBe(n+1);}
    const baseline=(await call({method:'status'})).state; const metrics:Metrics={acceptedRuns:0,errors:[]};let latest=baseline;
    // Epoch/latestInputRevision are v3 context, not fields invented on v2.
    // B08's actual boundary is the v2 plan-state/capability refusal.
    for(const event of c.events){only(event,['atMs','op']);expect(event.atMs).toBe(0);if(event.op!=='v2-prepare-error-aware')throw new Error(`unsupported v2 op ${event.op}`);
      const reply=await call({method:'prepare',graph:graph(c,'test.fixture-error-aware')});if(reply.error)metrics.errors.push(reply.error.code);if(reply.data?.state==='accepted')metrics.acceptedRuns++;latest=reply.state;expect(reply.ok).toBe(false);
    }
    metrics.planRevisionUnchanged=isDeepStrictEqual(baseline,latest);expect(latest.active.version).toBe(version);expect(existsSync(join(f.directory,'error-aware-executed'))).toBe(false);assertExpected(c,metrics,new Map());
  } finally {child.stdin.end();await exited;lines.close();cleanup(f.directory);}
}
it(IDS[0],formal_b03);it(IDS[1],formal_b07);it(IDS[2],formal_b08);
it('binds the complete formal selection and transport evidence without promoting Core coverage',()=>{
  const coverage=JSON.parse(readFileSync(resolve(ROOT,'tests/execution_coverage.json'),'utf8'));
  const expected=[{id:IDS[0],status:'fixture-http',name:'formal_b03'},{id:IDS[1],status:'fixture-http',name:'formal_b07'},{id:IDS[2],status:'fixture-v2-cli',name:'formal_b08'}];
  expect(coverage.cases.filter((c:RecordValue)=>c.transport).map((c:RecordValue)=>c.id).sort()).toEqual([...IDS].sort());
  for(const item of expected){
    selected(item.id);
    const entries=coverage.cases.filter((c:RecordValue)=>c.id===item.id);expect(entries).toHaveLength(1);
    const entry=entries[0]; expect(entry.status).toBe('related-only');expect(entry.transport.status).toBe(item.status);
    expect(entry.transport.evidence).toEqual([`editor/tests/execution-transport-fixtures.test.ts#${item.name}`]);
    const [path,name]=entry.transport.evidence[0].split('#');
    expect(readFileSync(resolve(ROOT,path),'utf8')).toContain(`async function ${name}(`);
    expect(entry.transport.boundary.length).toBeGreaterThan(0);
  }
});
