import { createServer, type Server } from 'node:http';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, rmSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge.ts';
import { ExecutionBroker, ExecutionSessionPool } from '../server/execution-broker.ts';
const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)), '..');
const CORE = resolve(ROOT,'target/debug',process.platform === 'win32' ? 'diy-ime2.exe' : 'diy-ime2');
const GATE = resolve(ROOT,'target/debug/examples',process.platform === 'win32' ? 'execution_broker_gate.exe' : 'execution_broker_gate');
const run = promisify(execFile);
function fixture() {
  const dir = mkdtempSync(join(tmpdir(),'diy-ime-v3-http-')), policyFile = join(dir,'policy.json');
  const policy = JSON.parse(readFileSync(resolve(ROOT,'tests/contracts/execution_control.json'),'utf8')).resourceProfiles.bounded;
  Object.assign(policy,{ maxPhysicalJobs:1, maxQueuedJobs:0, retentionMs:60000 }); writeFileSync(policyFile,JSON.stringify(policy));
  return { dir, policyFile, policy, lock:policyFile+'.broker-lock' };
}
function cleanup(dir: string) {
  const actual = realpathSync(dir), base = realpathSync(tmpdir());
  if (dirname(actual) !== base || !basename(actual).startsWith('diy-ime-v3-http-')) throw new Error('unexpected fixture directory');
  rmSync(actual,{recursive:true,force:true});
}
async function until(done: () => boolean) { const end = Date.now()+5000; while (!done()) { if (Date.now()>=end) throw new Error('Host did not settle'); await new Promise(resolve => setTimeout(resolve,5)); } }
function live(pid: number) { try { process.kill(pid,0); return true; } catch { return false; } }
function releaseGates(dir: string) { writeFileSync(join(dir,'producer-end'),'1'); writeFileSync(join(dir,'callback-return'),'1'); rmSync(join(dir,'hold-replies'),{ force:true }); }
async function serverFor(policyFile?: string, core = CORE, timeout: number | null = 5000) {
  const middleware = createApiMiddleware(policyFile === undefined ? {} : { execution:{ policyFile, core, root:ROOT, responseTimeoutMs:timeout } });
  let disconnected = 0;
  const server: Server = createServer((req,res) => { res.on('close', () => { if (!res.writableEnded) disconnected++; });middleware(req,res,() => {res.statusCode=404;res.end();}); });
  await new Promise<void>(resolve => server.listen(0,'127.0.0.1',resolve));
  const addr = server.address(); if (!addr || typeof addr === 'string') throw new Error('listen');
  const origin = `http://127.0.0.1:${addr.port}`;
  return { middleware, origin, get disconnected() { return disconnected; }, async close() { middleware.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); } };
}
async function post(origin: string,path: string,input: unknown = {},signal?: AbortSignal) {
  const response = await fetch(origin+path,{ method:'POST', headers:{ 'Content-Type':'application/json' }, body:JSON.stringify(input), signal });
  return { status:response.status, body:await response.json() };
}
async function open(origin: string) { const reply = await post(origin,'/api/v3/sessions'); expect(reply.status).toBe(200); return reply.body.data; }
const call = (origin: string,id: string,input: unknown,signal?: AbortSignal) => post(origin,`/api/v3/sessions/${id}/request`,input,signal);
async function prepare(origin: string,id: string,kind = 'input.text') {
  const prepared = await call(origin,id,{ method:'prepare',graph:{ version:1,nodes:[{ id:'source',kind,version:1 }],edges:[] } }); expect(prepared.status).toBe(200);
  const active = await call(origin,id,{ method:'activate',token:prepared.body.data.result.token }); expect(active.status).toBe(200);
}
const start = { method:'start-run',startKey:'18446744073709551615',expectedPlanVersion:'1',targets:[{ node:'source',port:'out' }] };
async function terminal(origin: string,id: string,key: unknown) {
  const end=Date.now()+5000;
  while (true) { const result=await call(origin,id,{method:'get-run',key}); expect(result.status).toBe(200); if(result.body.data.result.state==='terminal') return result.body.data.result; if(Date.now()>end) throw new Error('result timeout'); await new Promise(resolve=>setTimeout(resolve,1)); }
}
it('keeps v3 explicitly unconfigured and preserves the existing v2 HTTP default',async () => {
  const previous = process.env.DIY_IME_V3_RESOURCE_POLICY; delete process.env.DIY_IME_V3_RESOURCE_POLICY;
  const app=await serverFor();
  try {
    const missing=await post(app.origin,'/api/v3/sessions'); expect(missing.status).toBe(503); expect(missing.body.error.code).toBe('host-unconfigured');
    const old=await post(app.origin,'/api/sessions'); expect(old.status).toBe(200); expect(typeof old.body.data.state.revision).toBe('number');
    await fetch(`${app.origin}/api/sessions/${old.body.data.id}`,{method:'DELETE'});
  } finally { await app.close(); if(previous===undefined) delete process.env.DIY_IME_V3_RESOURCE_POLICY; else process.env.DIY_IME_V3_RESOURCE_POLICY=previous; }
});
it('uses one real broker for HTTP tabs/reconnect, preserves u64 strings and never restores an old epoch',async () => {
  const f=fixture(), app=await serverFor(f.policyFile);
  try {
    const a=await open(app.origin), b=await open(app.origin); const pid=JSON.parse(readFileSync(f.lock,'utf8')).pid;
    expect(a.resourceDomain).toBe(b.resourceDomain); expect(a.resourcePolicy).toEqual(f.policy); expect(a.id).not.toBe(b.id);
    await prepare(app.origin,a.id);
    expect((await fetch(`${app.origin}/api/v3/sessions/${b.id}`).then(res=>res.json())).data.state.activePlanVersion).toBeNull();
    const request={...start,input:{revision:1,text:'kya😀',mode:'compose'}};
    // Receive and discard the acceptance: then recover it only by get-start.
    await call(app.origin,a.id,request);
    const key=(await call(app.origin,a.id,{method:'get-start',startKey:start.startKey})).body.data.result.key;
    expect(key.planVersion).toBe('1'); expect(key.sessionEpoch).toBe(a.id);
    const result=await terminal(app.origin,a.id,key); expect(result.result.ports[0].value.packet.root.text).toBe('kya😀');
    expect((await call(app.origin,a.id,request)).body.data.result.replayed).toBe(true);
    expect((await call(app.origin,a.id,{...request,targets:[]})).body.error.code).toBe('start-conflict');
    const invalid=await call(app.origin,a.id,{method:'status',extra:true}); expect(invalid.status).toBe(422); expect(invalid.body.error.code).toBe('protocol-request'); expect(invalid.body.state.activePlanVersion).toBe('1');
    const invalidated=await call(app.origin,a.id,{method:'invalidate-input',expectedPlanVersion:'1',revision:2}); expect(invalidated.status).toBe(200);
    expect(invalidated.body.data.state.revision).toBe(invalid.body.state.revision); expect(invalidated.body.data.state.latestInputRevision).toBe(2);
    expect((await call(app.origin,a.id,{method:'get-run',key})).body.data.result.current).toBe(false);
    expect((await call(app.origin,a.id,{method:'get-start',startKey:start.startKey})).body.data.result.key).toEqual(key);
    await fetch(`${app.origin}/api/v3/sessions/${a.id}`,{method:'DELETE'}); const next=await open(app.origin);
    expect(next.id).not.toBe(a.id); expect(next.resourceDomain).toBe(a.resourceDomain); expect(next.state.activePlanVersion).toBeNull(); expect(JSON.parse(readFileSync(f.lock,'utf8')).pid).toBe(pid);
    expect((await call(app.origin,next.id,{method:'get-run',key})).body.error.code).toBe('session-conflict');
    expect((await call(app.origin,next.id,{method:'get-start',startKey:start.startKey})).body.error.code).toBe('start-not-seen');
    expect((await call(app.origin,b.id,{method:'hello'})).body.data.result.resourceDomain).toBe(a.resourceDomain);
    await open(app.origin); await open(app.origin); expect((await post(app.origin,'/api/v3/sessions')).body.error.code).toBe('session-limit');
    expect((await post(app.origin,'/api/v3/sessions',{policyFile:'ignored'})).body.error.code).toBe('protocol-request');
    expect((await call(app.origin,next.id,{method:'start-run',startKey:9007199254740992,expectedPlanVersion:'1',targets:[]})).body.error.code).toBe('protocol-request');
  } finally { await app.close(); await until(()=>!existsSync(f.lock)); cleanup(f.dir); }
});
it('shares a Host across middleware owners, preserves it after one closes and does not spawn for pre-send abort',async () => {
  const f=fixture(), cfg={core:CORE,root:ROOT,policyFile:f.policyFile,responseTimeoutMs:5000};
  const a=new ExecutionSessionPool(cfg), b=new ExecutionSessionPool(cfg);
  try {
    const aborted=new AbortController(); aborted.abort(); await expect(a.create(aborted.signal)).rejects.toMatchObject({code:'aborted'}); expect(a.hostPid).toBeUndefined(); expect(existsSync(f.lock)).toBe(false);
    const first=await a.create(), second=await b.create(); expect(a.hostPid).toBe(b.hostPid); expect(first.resourceDomain).toBe(second.resourceDomain);
    const pid=b.hostPid; a.close(); expect((await b.call(second.id,{method:'status'})).state.revision).toBe('0'); expect(b.hostPid).toBe(pid);
    await expect(b.call(second.id,{method:'status'},aborted.signal)).rejects.toMatchObject({code:'aborted'});
    await expect(b.call(second.id,{method:'prepare',graph:'x'.repeat(1024*1024)})).rejects.toMatchObject({code:'request-limit'});
    expect((await b.call(second.id,{method:'status'})).state.revision).toBe('0');
  } finally { a.close(); b.close(); if(b.hostExited) await b.hostExited; await until(()=>!existsSync(f.lock)); cleanup(f.dir); }
});
it('rejects malformed/bounded broker output without killing or automatically restarting the child',async () => {
  for(const bad of [Buffer.from('{"broker":2,"id":"1","ok":true,"data":{}}\n'),Buffer.alloc(8*1024*1024+2,120)]) {
    const f=fixture(), broker=new ExecutionBroker({core:CORE,root:ROOT,policyFile:f.policyFile,responseTimeoutMs:5000});
    try {
      const opened=await broker.request({method:'open-session'}); expect(opened.ok).toBe(true); const pid=broker.child.pid;
      broker.child.stdout.emit('data',bad);
      await expect(broker.request({method:'open-session'})).rejects.toMatchObject({code:'host-lost'}); expect(broker.child.pid).toBe(pid);
      expect((await broker.exited).code).toBe(0);
    } finally { broker.shutdown(); await broker.exited; await until(()=>!existsSync(f.lock)); cleanup(f.dir); }
  }
});
it('closes only the HTTP Session on sent-request abort; keeps producer ownership and blocks Host replacement until true ending',async () => {
  const f=fixture(), previous=process.env.DIY_IME_BROKER_TEST_GATE; process.env.DIY_IME_BROKER_TEST_GATE=f.dir;
  const app=await serverFor(f.policyFile,GATE);
  try {
    const a=await open(app.origin), sibling=await open(app.origin); await prepare(app.origin,a.id,'test.http-held');
    const accepted=await call(app.origin,a.id,start); expect(accepted.status).toBe(200); await until(()=>existsSync(join(f.dir,'started')));
    const pid=JSON.parse(readFileSync(f.lock,'utf8')).pid;
    writeFileSync(join(f.dir,'hold-replies'),'1'); const abort=new AbortController();
    const pending=call(app.origin,a.id,{method:'status'},abort.signal); const abandoned=expect(pending).rejects.toThrow();
    await until(()=>existsSync(join(f.dir,'reply-held'))); abort.abort(); await abandoned;
    // Client rejection precedes TCP close delivery. Observe the actual server close
    // before releasing its reply, otherwise the response can win and cancel no Session.
    await until(()=>app.disconnected > 0);
    rmSync(join(f.dir,'hold-replies'),{force:true}); await until(()=>existsSync(join(f.dir,'cancelled')));
    expect((await call(app.origin,sibling.id,{method:'hello'})).body.data.result.resourceDomain).toBe(a.resourceDomain);
    const fresh=await open(app.origin); expect(fresh.id).not.toBe(a.id); expect(fresh.resourceDomain).toBe(a.resourceDomain); await prepare(app.origin,fresh.id,'test.http-held');
    const busy=await call(app.origin,fresh.id,start); expect(busy.status).toBe(503); expect(busy.body.error.code).toBe('busy'); expect(readFileSync(join(f.dir,'starts.log'),'utf8').trim().split('\n')).toHaveLength(1);
    app.middleware.close(); expect(live(pid)).toBe(true); expect(existsSync(f.lock)).toBe(true);
    const replacement = await run(CORE,['broker-v3',f.policyFile,f.lock],{cwd:ROOT}).then(()=>({failed:false}),()=>({failed:true})); expect(replacement.failed).toBe(true); expect(live(pid)).toBe(true);
    writeFileSync(join(f.dir,'producer-end'),'1'); await until(()=>existsSync(join(f.dir,'producer-ended'))); expect(live(pid)).toBe(true); expect(existsSync(f.lock)).toBe(true);
    writeFileSync(join(f.dir,'callback-return'),'1'); await until(()=>!existsSync(f.lock)); await until(()=>!live(pid));
  } finally { releaseGates(f.dir); await app.close(); await until(()=>!existsSync(f.lock)); if(previous===undefined) delete process.env.DIY_IME_BROKER_TEST_GATE; else process.env.DIY_IME_BROKER_TEST_GATE=previous; cleanup(f.dir); }
});
it('treats finite response monitoring as Session loss, not a run deadline or Host kill',async () => {
  const f=fixture(), previous=process.env.DIY_IME_BROKER_TEST_GATE; process.env.DIY_IME_BROKER_TEST_GATE=f.dir;
  // This selected RPC monitor also covers cold Windows process startup.
  // Keep the tested response-loss semantics independent of a 100ms boot race.
  const monitorMs=1000, app=await serverFor(f.policyFile,GATE,monitorMs);
  try {
    const a=await open(app.origin), b=await open(app.origin); const pid=JSON.parse(readFileSync(f.lock,'utf8')).pid;
    writeFileSync(join(f.dir,'hold-replies'),'1'); const timed=await call(app.origin,a.id,{method:'status'}); expect(timed.status).toBe(503); expect(timed.body.error.code).toBe('session-lost'); expect(timed.body.state).toBeUndefined();
    expect(live(pid)).toBe(true); rmSync(join(f.dir,'hold-replies'),{force:true});
    expect((await call(app.origin,b.id,{method:'hello'})).status).toBe(200); expect(JSON.parse(readFileSync(f.lock,'utf8')).pid).toBe(pid);
    expect((await call(app.origin,a.id,{method:'status'})).body.error.code).toBe('session-lost');
  } finally { releaseGates(f.dir); await app.close(); await until(()=>!existsSync(f.lock)); if(previous===undefined) delete process.env.DIY_IME_BROKER_TEST_GATE; else process.env.DIY_IME_BROKER_TEST_GATE=previous; cleanup(f.dir); }
});
it('rejects an existing ownership file without deleting it or creating a new Host',async () => {
  const f=fixture(), app=await serverFor(f.policyFile); writeFileSync(f.lock,'stale-owner');
  try { const failed=await post(app.origin,'/api/v3/sessions'); expect(failed.status).toBe(503); expect(failed.body.error.code).toBe('host-lost'); expect(readFileSync(f.lock,'utf8')).toBe('stale-owner'); }
  finally { await app.close(); cleanup(f.dir); }
});
it('keeps healthy long-running work past the response monitoring interval until explicit cancellation',async () => {
  const f=fixture(), previous=process.env.DIY_IME_BROKER_TEST_GATE; process.env.DIY_IME_BROKER_TEST_GATE=f.dir;
  // This selected RPC monitor also covers cold Windows process startup.
  // Keep the tested response-loss semantics independent of a 100ms boot race.
  const monitorMs=1000, app=await serverFor(f.policyFile,GATE,monitorMs);
  try {
    const a=await open(app.origin); await prepare(app.origin,a.id,'test.http-held');
    const accepted=await call(app.origin,a.id,start); const key=accepted.body.data.result.key;
    await until(()=>existsSync(join(f.dir,'started')));
    for(let i=0;i<4;i++) {
      // Four waits exceed one full monitor interval while the producer stays live.
      await new Promise(resolve=>setTimeout(resolve,Math.ceil(monitorMs/3)));
      const snapshot=await call(app.origin,a.id,{method:'get-run',key}); expect(snapshot.status).toBe(200); expect(snapshot.body.data.result.state).toBe('running');
    }
    expect(existsSync(join(f.dir,'cancelled'))).toBe(false);
    const cancelled=await call(app.origin,a.id,{method:'cancel-run',key}); expect(cancelled.body.data.result.result.reason).toBe('cancelled'); expect(existsSync(f.lock)).toBe(true);
  } finally { releaseGates(f.dir); await app.close(); await until(()=>!existsSync(f.lock)); if(previous===undefined) delete process.env.DIY_IME_BROKER_TEST_GATE; else process.env.DIY_IME_BROKER_TEST_GATE=previous; cleanup(f.dir); }
});
it('bounds pending broker requests while reserving control slots to close all four Sessions',async () => {
  const f=fixture(), previous=process.env.DIY_IME_BROKER_TEST_GATE; process.env.DIY_IME_BROKER_TEST_GATE=f.dir;
  const broker=new ExecutionBroker({core:GATE,root:ROOT,policyFile:f.policyFile,responseTimeoutMs:null});
  try {
    const opened=await Promise.all(Array.from({length:4},()=>broker.request({method:'open-session'}))); const sessions=opened.map(reply=>reply.data as {sessionEpoch:string}); const data=sessions[0]; const pid=broker.child.pid;
    writeFileSync(join(f.dir,'hold-replies'),'1');
    const pending=Array.from({length:12},(_,i)=>broker.request({method:'request',sessionEpoch:data.sessionEpoch,message:{protocol:3,id:String(i+1),request:{method:'status'}}}));
    await until(()=>existsSync(join(f.dir,'reply-held')));
    await expect(broker.request({method:'open-session'})).rejects.toMatchObject({code:'busy'});
    const closing=sessions.map(session=>broker.request({method:'close-session',sessionEpoch:session.sessionEpoch},true));
    await expect(broker.request({method:'close-session',sessionEpoch:data.sessionEpoch},true)).rejects.toMatchObject({code:'busy'});
    rmSync(join(f.dir,'hold-replies'),{force:true});
    expect((await Promise.all(pending)).every(reply=>reply.ok)).toBe(true);
    const replies=await Promise.all(closing); expect(replies.every(reply=>reply.ok)).toBe(true);
    expect((await broker.request({method:'open-session'})).ok).toBe(true); expect(broker.child.pid).toBe(pid);
  } finally { releaseGates(f.dir); broker.shutdown(); await broker.exited; await until(()=>!existsSync(f.lock)); if(previous===undefined) delete process.env.DIY_IME_BROKER_TEST_GATE; else process.env.DIY_IME_BROKER_TEST_GATE=previous; cleanup(f.dir); }
});
