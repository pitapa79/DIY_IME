import { createServer, type Server } from 'node:http';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
const state = vi.hoisted(() => ({ constructors: [] as unknown[], calls: [] as { operation: Record<string, unknown>; payload?: Buffer }[], cancel: 0, review: undefined as undefined | Promise<unknown> }));
vi.mock('../server/node-packages.ts', () => ({
  NODE_MANAGER_CONFIG_ENV: 'TEST_NODE_MANAGER_CONFIG',
  PackageManager: class {
    uploadBytes = 128;
    constructor(options: unknown) { state.constructors.push(options); }
    call(operation: Record<string, unknown>, payload?: Buffer) {
      state.calls.push({ operation, payload });
      return operation.method === 'review' ? state.review ?? Promise.resolve({ reviewId: 'a'.repeat(32) }) : Promise.resolve({ catalog: { revision: '1', entries: [] } });
    }
    cancelCurrent() { state.cancel++; }
    close() { return Promise.resolve(); }
  },
}));
let server: Server, close: () => void, origin: string;
async function open(configured = true) {
  const middleware = createApiMiddleware(configured ? { packages: { executable: 'fixed-executable', config: 'host-config' } } : {});
  close = middleware.close;
  server = createServer((request, response) => middleware(request, response, () => { response.writeHead(404); response.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('missing listener');
  origin = 'http://127.0.0.1:' + address.port;
}
const post = (path: string, value: unknown) => fetch(origin + '/api/node-packages/' + path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value) });
beforeEach(() => { state.constructors.length = 0; state.calls.length = 0; state.cancel = 0; state.review = undefined; });
afterEach(async () => { close?.(); server?.closeAllConnections(); if (server) await new Promise<void>(resolve => server.close(() => resolve())); });
it('passes exact binary CODE payload only to the fixed Host and refuses browser paths/authority', async () => {
  await open(); expect(state.constructors).toEqual([{ executable: 'fixed-executable', config: 'host-config' }]);
  const bytes = Buffer.from([0, 255, 0, 12]);
  const response = await fetch(origin + '/api/node-packages/review', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: bytes });
  expect(response.status).toBe(200); expect(state.calls[0]).toEqual({ operation: { method: 'review' }, payload: bytes });
  const count = state.calls.length;
  for (const extra of [{ path: 'private' }, { trusted: true }, { permissions: ['*'] }, { root: 'elsewhere' }]) {
    expect((await post('commit', { reviewId: 'a'.repeat(32), allowDowngrade: false, allowSameVersionRebuild: false, ...extra })).status).toBe(422);
  }
  expect(state.calls).toHaveLength(count);
  expect((await post('commit', { reviewId: 'a'.repeat(32), allowDowngrade: true, allowSameVersionRebuild: false })).status).toBe(200);
  expect(state.calls.at(-1)?.operation).toEqual({ method: 'commit', reviewId: 'a'.repeat(32), allowDowngrade: true, allowSameVersionRebuild: false });
});
it('checks origin, type and size before any Host review', async () => {
  await open();
  expect((await fetch(origin + '/api/node-packages', { headers: { Origin: 'https://outside.invalid' } })).status).toBe(403);
  expect((await post('review', { path: 'private-file' })).status).toBe(415);
  expect((await fetch(origin + '/api/node-packages/review', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: Buffer.alloc(129) })).status).toBe(413);
  expect(state.calls).toHaveLength(0);
});
it('keeps HTTP ownership busy after cancellation until the original Host operation settles', async () => {
  await open(); let settle!: (value: unknown) => void;
  state.review = new Promise(resolve => { settle = resolve; });
  const pending = fetch(origin + '/api/node-packages/review', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: Buffer.from([0]) });
  await vi.waitFor(() => expect(state.calls).toHaveLength(1));
  expect((await fetch(origin + '/api/node-packages')).status).toBe(409);
  expect((await post('cancel', {})).status).toBe(200); expect(state.cancel).toBe(1);
  expect((await fetch(origin + '/api/node-packages')).status).toBe(409);
  settle({ settled: true }); await pending;
  expect((await fetch(origin + '/api/node-packages')).status).toBe(200);
});
it('returns unavailable state without starting an unconfigured Host', async () => {
  await open(false);
  expect((await (await fetch(origin + '/api/node-packages')).json()).data).toEqual({ available: false });
  expect((await post('review', {})).status).toBe(503); expect(state.constructors).toHaveLength(0);
});