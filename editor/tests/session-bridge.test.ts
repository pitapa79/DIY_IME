import { createServer, type Server } from 'node:http';
import { once } from 'node:events';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
import { afterAll, afterEach, beforeAll, expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
import { PersistentCore } from '../server/session';

const sample = JSON.parse(readFileSync(new URL('../../examples/graphs/hello.json', import.meta.url), 'utf8'));
let server: Server, middleware: ReturnType<typeof createApiMiddleware>, origin: string;
const ids = new Set<string>();
beforeAll(async () => {
  middleware = createApiMiddleware({ legacy: { core: resolve(process.cwd(), '../target/debug', process.platform === 'win32' ? 'diy-ime2-test-host.exe' : 'diy-ime2-test-host'), root: resolve(process.cwd(), '..') } }); server = createServer((req, res) => middleware(req, res, () => res.end()));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('address'); origin = `http://127.0.0.1:${address.port}`;
});
afterEach(async () => { await Promise.all([...ids].map(id => fetch(`${origin}/api/sessions/${id}`, { method: 'DELETE' }))); ids.clear(); });
afterAll(async () => { middleware.close(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); });
async function post(path: string, input: unknown = {}) {
  const response = await fetch(origin + path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(input) }); return { status: response.status, body: await response.json() };
}
async function create() { const result = await post('/api/sessions'); expect(result.status).toBe(200); ids.add(result.body.data.id); return result.body.data.id as string; }
const call = (id: string, method: string, input: unknown = {}) => post(`/api/sessions/${id}/${method}`, input);
async function activate(id: string, graph = sample) {
  const prepared = await call(id, 'prepare', { graph }); expect(prepared.body.ok).toBe(true); return call(id, 'activate', { token: prepared.body.data.state.prepared.token });
}
const text = (response: any) => response.body.data.result.nodes.result.outputs.out.packet.root.text;

it('retains an engine across calls, preserves invalid edits, switches and rolls back', async () => {
  const id = await create(); await activate(id);
  const first = await call(id, 'run', { expectedPlanVersion: 1 }), second = await call(id, 'run', { expectedPlanVersion: 1 });
  expect(text(first)).toBe('HELLO 𠮷'); expect(second.body.data.result.runId).toBeGreaterThan(first.body.data.result.runId);
  const edited = structuredClone(sample); edited.nodes[0].config.packet.root = { type: 'text', text: 'updated' };
  const prepared = await call(id, 'prepare', { graph: edited }); expect(text(await call(id, 'run', { expectedPlanVersion: 1 }))).toBe('HELLO 𠮷');
  const invalid = await call(id, 'prepare', { graph: { ...edited, edges: [] } }); expect(invalid.status).toBe(422); expect(invalid.body.error.code).toBe('missing-input'); expect(invalid.body.state.active.version).toBe(1);
  const token = prepared.body.data.state.prepared.token; expect(invalid.body.state.prepared.token).toBe(token);
  await call(id, 'activate', { token }); expect(text(await call(id, 'run', { expectedPlanVersion: 2 }))).toBe('UPDATED');
  const restored = await call(id, 'rollback', { expectedPlanVersion: 2 }); expect(restored.body.data.state.active.version).toBe(3); expect(text(await call(id, 'run', { expectedPlanVersion: 3 }))).toBe('HELLO 𠮷');
});

it('passes runtime snapshots over protocol 2 and preserves plan state across valid and invalid inputs', async () => {
  const id=await create();
  const graph=JSON.parse(readFileSync(new URL('../../examples/graphs/runtime-input.json',import.meta.url),'utf8'));
  await activate(id,graph); const initial=(await fetch(`${origin}/api/sessions/${id}`).then(res=>res.json())).data.state;
  for (const [revision,value,mode] of [[1,'k','compose'],[2,'kya','finalize'],[3,'','compose'],[4,'𠮷30%','finalize']] as const) {
    const response=await call(id,'run',{expectedPlanVersion:1,input:{revision,text:value,mode}});
    expect(response.status).toBe(200); expect(response.body.data.result.succeeded).toBe(true);
    expect(text(response)).toBe(value); expect(response.body.data.result.inputRevision).toBe(revision);
    expect(response.body.data.state).toEqual(initial);
  }
  for (const input of [null,{revision:0,text:'a',mode:'compose'},{revision:5,text:'a',mode:'wrong'},{revision:5,text:'a',mode:'compose',extra:true}]) {
    const response=await call(id,'run',{expectedPlanVersion:1,input}); expect(response.status).toBe(422);
    expect(response.body.error.code).toBe('invalid-runtime-input'); expect(response.body.state).toEqual(initial);
  }
  const stale=await call(id,'run',{expectedPlanVersion:0,input:null}); expect(stale.body.error.code).toBe('plan-conflict');
  const missing=await call(id,'run',{expectedPlanVersion:1}); expect(missing.body.data.result.succeeded).toBe(false);
  expect(missing.body.data.result.nodes.input.diagnostic.error.code).toBe('runtime-input-missing');
  const other=await create(); await activate(other,graph);
  expect((await call(other,'run',{expectedPlanVersion:1})).body.data.result.succeeded).toBe(false);
});
it('isolates tabs and rejects stale tokens and plan versions', async () => {
  const a = await create(), b = await create();
  const replies = await Promise.all([call(a, 'prepare', { graph: sample }), call(a, 'prepare', { graph: sample })]); replies.sort((x, y) => x.body.data.state.revision - y.body.data.state.revision);
  expect(replies[0].body.data.state.prepared.token).not.toBe(replies[1].body.data.state.prepared.token);
  expect((await call(a, 'activate', { token: replies[0].body.data.state.prepared.token })).body.error.code).toBe('stale-preparation');
  await call(a, 'activate', { token: replies[1].body.data.state.prepared.token }); expect((await call(a, 'run', { expectedPlanVersion: 0 })).body.error.code).toBe('plan-conflict');
  expect((await fetch(`${origin}/api/sessions/${b}`).then(res => res.json())).data.state.active).toBeNull(); expect((await call(b, 'run', { expectedPlanVersion: 1 })).body.error.code).toBe('no-active-plan');
});


it('retains the active plan after malformed graph shapes and parser depth errors', async () => {
  const id = await create(); await activate(id);
  const bad = await call(id, 'prepare', { graph: { ...sample, unknown: true } }); expect(bad.status).toBe(422); expect(bad.body.error.code).toBe('protocol-request');
  let nested: unknown = 'deep'; for (let i = 0; i < 150; i++) nested = [nested]; const deep = structuredClone(sample); deep.nodes[0].config.extra = nested;
  const response = await call(id, 'prepare', { graph: deep }); expect(response.status).toBe(422); expect(response.body.error.code).toBe('protocol-json'); expect(text(await call(id, 'run', { expectedPlanVersion: 1 }))).toBe('HELLO 𠮷');
});
it('closes sessions without recreating them and starts a fresh session empty', async () => {
  const id = await create(); await activate(id); await fetch(`${origin}/api/sessions/${id}`, { method: 'DELETE' });
  const missing = await call(id, 'run', { expectedPlanVersion: 1 }); expect(missing.status).toBe(503); expect(missing.body.error.code).toBe('session-lost');
  const next = await create(); expect(next).not.toBe(id); expect((await fetch(`${origin}/api/sessions/${next}`).then(res => res.json())).data.state.active).toBeNull();
});
it('limits session capacity and releases a slot when a tab closes', async () => {
  const opened = await Promise.all([create(), create(), create(), create()]);
  const full = await post('/api/sessions'); expect(full.status).toBe(503); expect(full.body.error.code).toBe('session-limit');
  await fetch(`${origin}/api/sessions/${opened[0]}`, { method: 'DELETE' });
  expect(await create()).not.toBe(opened[0]);
});
it('preserves a worker for pre-send cancellation and detects a terminated worker', async () => {
  const root = resolve(process.cwd(), '..'), core = resolve(root, 'target/debug', process.platform === 'win32' ? 'diy-ime2-test-host.exe' : 'diy-ime2-test-host'); const worker = new PersistentCore(core, root);
  try {
    await worker.request({ method: 'status' }); const pid = worker.child.pid; const controller = new AbortController(); controller.abort();
    await expect(worker.request({ method: 'status' }, controller.signal)).rejects.toThrow('送信前'); expect((await worker.request({ method: 'status' })).ok).toBe(true); expect(worker.child.pid).toBe(pid);
    const closed = once(worker.child, 'close'); worker.child.kill(); await closed; await expect(worker.request({ method: 'status' })).rejects.toThrow('接続');
  } finally { worker.close(); }
});

