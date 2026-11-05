import { createServer, type Server } from 'node:http';
import { afterEach, expect, it, vi } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
const state = vi.hoisted(() => ({ instances: [] as any[], hold: undefined as Promise<unknown> | undefined, ending: undefined as Promise<void> | undefined }));
vi.mock('../server/node-packages.ts', () => ({ NODE_MANAGER_CONFIG_ENV: 'TEST_PACKAGE_CONFIG', PackageManager: class {
  cleanExit = true; calls: any[] = []; cancelCurrent = vi.fn();
  constructor(public options: unknown, public role: string) { state.instances.push(this); }
  call(operation: any, payload: unknown, signal: unknown, control: boolean) { this.calls.push({ operation, payload, signal, control });return operation.method === 'run' ? state.hold : Promise.resolve(operation.method === 'status' ? { phase: 'empty' } : { protected: true }); }
  close() { return state.ending ?? Promise.resolve(); }
} }));
let server: Server, middleware: ReturnType<typeof createApiMiddleware>, origin: string;
async function open() {
  state.instances.length = 0;state.hold = undefined;state.ending = undefined;
  middleware = createApiMiddleware({ external: { executable: 'fixed-host', config: 'fixed-config' } });
  server = createServer((req, res) => middleware(req, res, () => { res.statusCode = 404;res.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const address = server.address();if (!address || typeof address === 'string') throw Error('listen');origin = 'http://127.0.0.1:' + address.port;
}
async function post(path: string, value: unknown = {}, extra: Record<string, string> = {}) { const response = await fetch(origin + '/api/external/connections' + path, { method: 'POST', headers: { 'Content-Type': 'application/json', ...extra }, body: JSON.stringify(value) });return { status: response.status, body: await response.json() }; }
async function connect() { const result = await post('');expect(result.status).toBe(200);return result.body.data.connectionId as string; }
afterEach(async () => { if (middleware) await middleware.close();server?.closeAllConnections();if (server) await new Promise<void>(resolve => server.close(() => resolve())); });
it('accepts only exact operations through a fixed Host and refuses browser authority or another origin', async () => {
  await open();expect((await post('', { config: 'elsewhere' })).status).toBe(422);const id = await connect();const manager = state.instances[0];expect(manager.options).toEqual({ executable: 'fixed-host', config: 'fixed-config' });expect(manager.role).toBe('execution');
  for (const operation of [{ method: 'close' }, { method: 'protection', enabled: false, automatic: false }, { method: 'prepare', graph: {}, worker: 'other' }, { method: 'cancel', requestId: 'other-owner' }]) expect((await post('/' + id + '/request', operation)).status).toBe(422);
  expect((await post('/' + id + '/request', { method: 'status' }, { Origin: 'https://outside.invalid' })).status).toBe(403);expect(manager.calls).toHaveLength(1);
});
it('allows explicit detailed metadata recording but rejects untyped additions',async()=>{
 await open();const id=await connect();
 expect((await post('/'+id+'/request',{method:'traceStartDetailed'})).status).toBe(200);
 expect(state.instances[0].calls.at(-1).operation).toEqual({method:'traceStartDetailed'});
 for(const extra of [{text:'private'},{worker:'other'},{detailed:true}])expect((await post('/'+id+'/request',{method:'traceStartDetailed',...extra})).status).toBe(422);
});
it('fences late data when a protection control races an ordinary request', async () => {
  await open();const id = await connect();let finish!: (value: unknown) => void;state.hold = new Promise(resolve => { finish = resolve; });
  const request = post('/' + id + '/request', { method: 'run', request: {} });await vi.waitFor(() => expect(state.instances[0].calls).toHaveLength(2));
  expect((await post('/' + id + '/request', { method: 'protection', enabled: true })).status).toBe(200);expect(state.instances[0].calls.at(-1).control).toBe(true);
  finish({ text: 'private-old-data' });const reply = await request;expect(reply.status).toBe(409);expect(JSON.stringify(reply.body)).not.toContain('private-old-data');
});
it('holds ownership until actual ending and refuses old cleanup against a newer opaque lease', async () => {
  await open();const old = await connect();let end!: () => void;state.ending = new Promise(resolve => { end = resolve; });
  const removal = fetch(origin + '/api/external/connections/' + old, { method: 'DELETE' });await vi.waitFor(() => expect(state.instances).toHaveLength(1));
  await vi.waitFor(async () => expect((await post('')).status).toBe(409));end();expect((await removal).status).toBe(200);state.ending = undefined;
  const next = await connect();expect(next).not.toBe(old);expect((await fetch(origin + '/api/external/connections/' + old, { method: 'DELETE' })).status).toBe(200);
  expect((await post('/' + next + '/request', { method: 'status' })).status).toBe(200);expect((await post('/' + old + '/request', { method: 'status' })).status).toBe(409);
});
it('never respawns an abnormally ended Host with a fresh connection', async () => {
  await open();const id = await connect();state.instances[0].cleanExit = false;
  await fetch(origin + '/api/external/connections/' + id, { method: 'DELETE' });expect((await post('')).status).toBe(503);expect(state.instances).toHaveLength(1);
});
it('permits only secret metadata/write/delete operations and fences an older result when secret rotation begins', async () => {
  await open();const id = await connect();
  for (const operation of [{ method: 'secretRead', id: 'service' }, { method: 'secrets', root: 'other' }, { method: 'secretPut', id: 'service', value: 'synthetic', executable: 'other' }]) expect((await post('/' + id + '/request', operation)).status).toBe(422);
  let finish!: (value: unknown) => void;state.hold = new Promise(resolve => { finish = resolve; });const old = post('/' + id + '/request', { method: 'run', request: {} });
  await vi.waitFor(() => expect(state.instances[0].calls).toHaveLength(2));expect((await post('/' + id + '/request', { method: 'secretPut', id: 'service', value: 'synthetic' })).status).toBe(200);
  expect(state.instances[0].calls.at(-1).control).toBe(false);finish({ text: 'private-old-result' });const withdrawn = await old;expect(withdrawn.status).toBe(409);expect(JSON.stringify(withdrawn.body)).not.toContain('private-old-result');
});
