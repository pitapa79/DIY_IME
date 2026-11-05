import { ExecutionBroker } from '../server/execution-broker';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PersistentCore, SessionPool, readResponseTimeout, RESPONSE_TIMEOUT_ENV, MAX_RESPONSE_TIMEOUT_MS } from '../server/session';
import { createApiMiddleware, invokeCore } from '../server/bridge';

const { spawn } = vi.hoisted(() => ({ spawn: vi.fn() }));
vi.mock('node:child_process', () => ({ spawn }));
class FakeChild extends EventEmitter {
  stdin = new PassThrough(); stdout = new PassThrough(); stderr = new PassThrough();
  kill = vi.fn(() => true);
  lines() { return this.stdin.read()?.toString().trim().split('\n').map((line: string) => JSON.parse(line)) ?? []; }
  reply(id: number, extra = {}) { this.stdout.emit('data', Buffer.from(JSON.stringify({ protocol: 2, id, ok: true, state: { revision: 0, active: null, prepared: null, canRollback: false }, ...extra }) + '\n')); }
}
let children: FakeChild[], cleanup: (() => void)[];
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
function worker(timeout?: number | null) {
  const core = new PersistentCore('fake', '.', undefined, timeout);
  cleanup.push(() => core.close()); return { core, child: children.at(-1)! };
}
function outcome<T>(promise: Promise<T>) { return promise.then(value => ({ value, error: undefined }), error => ({ value: undefined, error })); }
beforeEach(() => {
  children = []; cleanup = []; vi.useFakeTimers();
  spawn.mockImplementation(() => { const child = new FakeChild(); children.push(child); return child; });
});
afterEach(async () => { cleanup.reverse().forEach(close => close()); await flush(); vi.useRealTimers(); vi.unstubAllEnvs(); spawn.mockReset(); });

describe('host response timeout configuration', () => {
  it('uses the existing default and accepts finite boundaries and unlimited', () => {
    expect(readResponseTimeout(undefined)).toBe(10_000);
    expect(readResponseTimeout('1')).toBe(1);
    expect(readResponseTimeout('60000')).toBe(60_000);
    expect(readResponseTimeout('2147483647')).toBe(MAX_RESPONSE_TIMEOUT_MS);
    expect(readResponseTimeout('unlimited')).toBeNull();
  });
  it.each(['', '0', '-1', '+1', '1.5', '1e3', '0x10', '01', ' 1', '1 ', '1\n', 'Infinity', 'NaN', 'Unlimited', '2147483648', '9007199254740991', '9007199254740993', '9'.repeat(400)])('rejects invalid environment value %j at startup', value => {
    expect(() => readResponseTimeout(value)).toThrow(RESPONSE_TIMEOUT_ENV);
    vi.stubEnv(RESPONSE_TIMEOUT_ENV, value);
    expect(() => createApiMiddleware()).toThrow(RESPONSE_TIMEOUT_ENV);
    expect(spawn).not.toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
  });
  it.each([0, -1, 1.5, Infinity, NaN, MAX_RESPONSE_TIMEOUT_MS + 1])('rejects invalid numeric timeout %s before spawning or scheduling', value => {
    expect(() => new PersistentCore('fake', '.', undefined, value)).toThrow(RESPONSE_TIMEOUT_ENV);
    expect(() => new SessionPool('fake', '.', value)).toThrow(RESPONSE_TIMEOUT_ENV);
    expect(spawn).not.toHaveBeenCalled(); expect(vi.getTimerCount()).toBe(0);
  });
});

it.each([undefined, 60_000, MAX_RESPONSE_TIMEOUT_MS])('closes at the finite deadline %s and rejects queued work without retry', async timeout => {
  const { core, child } = worker(timeout); const duration = timeout ?? 10_000;
  const first = outcome(core.request({ method: 'run' })); const queued = outcome(core.request({ method: 'status' }));
  await flush(); expect(child.lines()).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(duration - 1); expect(child.kill).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect((await first).error).toMatchObject({ code: 'session-lost' });
  expect((await queued).error).toMatchObject({ code: 'session-lost' });
  expect(child.lines()).toHaveLength(0); expect(child.kill).toHaveBeenCalledOnce();
  expect(core.hasRequests).toBe(false); expect(vi.getTimerCount()).toBe(0);
});

it('clears each deadline after a response and gives the next request its own full timeout', async () => {
  const { core, child } = worker(60_000);
  const first = core.request({ method: 'run' }); const second = outcome(core.request({ method: 'status' }));
  await flush(); const [sent] = child.lines(); await vi.advanceTimersByTimeAsync(59_999);
  child.reply(sent.id); await first; await flush(); expect(child.lines()).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(59_999); expect(child.kill).not.toHaveBeenCalled();
  child.reply(2); expect((await second).value?.ok).toBe(true);
  expect(vi.getTimerCount()).toBe(0); await vi.advanceTimersByTimeAsync(60_000); expect(child.kill).not.toHaveBeenCalled();
});

it('keeps unlimited requests alive and preserves queue limits and pre-send cancellation', async () => {
  const { core, child } = worker(null);
  const first = core.request({ method: 'run' }); const controller = new AbortController();
  const canceled = outcome(core.request({ method: 'status' }, controller.signal)); controller.abort();
  const queued = Array.from({ length: 14 }, () => outcome(core.request({ method: 'status' })));
  await expect(core.request({ method: 'status' })).rejects.toMatchObject({ code: 'busy' });
  await flush(); expect(child.lines()).toHaveLength(1); expect(vi.getTimerCount()).toBe(0);
  await vi.advanceTimersByTimeAsync(24 * 60 * 60_000); expect(child.kill).not.toHaveBeenCalled();
  child.reply(1); await first; await flush(); expect((await canceled).error).toMatchObject({ code: 'aborted' });
  expect(child.lines()).toHaveLength(1); core.close();
  for (const reply of await Promise.all(queued)) expect(reply.error).toMatchObject({ code: 'session-lost' });
  expect(core.hasRequests).toBe(false); expect(child.kill).toHaveBeenCalledOnce();
});

it('preserves post-send cancellation and rejects waiting work with unlimited timeout', async () => {
  const { core, child } = worker(null); const controller = new AbortController();
  const first = outcome(core.request({ method: 'run' }, controller.signal)); const next = outcome(core.request({ method: 'status' }));
  await flush(); expect(child.lines()).toHaveLength(1); controller.abort();
  expect((await first).error).toMatchObject({ code: 'session-lost' }); expect((await next).error).toMatchObject({ code: 'session-lost' });
  expect(child.lines()).toHaveLength(0); expect(child.kill).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0);
});

it.each(['exit', 'spawn-error', 'stdin-error', 'json', 'protocol', 'id', 'shape', 'output-limit'])('preserves unlimited cleanup on %s', async failure => {
  const { core, child } = worker(null); const first = outcome(core.request({ method: 'run' })); const queued = outcome(core.request({ method: 'status' }));
  await flush(); child.lines();
  if (failure === 'exit') child.emit('close', 1);
  else if (failure === 'spawn-error') child.emit('error', new Error('failed spawn'));
  else if (failure === 'stdin-error') child.stdin.emit('error', new Error('failed write'));
  else if (failure === 'json') child.stdout.emit('data', Buffer.from('invalid\n'));
  else if (failure === 'protocol') child.reply(1, { protocol: 1 });
  else if (failure === 'id') child.reply(2);
  else if (failure === 'shape') child.reply(1, { state: {} });
  else child.stdout.emit('data', Buffer.alloc(8 * 1024 * 1024 + 1));
  expect((await first).error).toMatchObject({ code: failure === 'spawn-error' ? 'core-unavailable' : 'session-lost' });
  expect((await queued).error).toMatchObject({ code: 'session-lost' });
  expect(child.kill).toHaveBeenCalledOnce(); expect(child.lines()).toHaveLength(0); expect(core.hasRequests).toBe(false);
});

it('preserves request capacity and framed parser errors in unlimited mode', async () => {
  const { core, child } = worker(null);
  await expect(core.request({ data: 'x'.repeat(1024 * 1024) })).rejects.toMatchObject({ code: 'request-limit' });
  expect(child.lines()).toHaveLength(0); expect(child.kill).not.toHaveBeenCalled();
  const parsed = core.request({ method: 'status' }); await flush();
  child.reply(0, { id: null, ok: false, error: { code: 'protocol-json', message: 'bad frame' } });
  expect((await parsed).ok).toBe(false); expect(vi.getTimerCount()).toBe(0); expect(child.kill).not.toHaveBeenCalled();
});

it.each([null, 30 * 60_000])('keeps running and queued sessions out of idle expiry with timeout %s', async timeout => {
  const pool = new SessionPool('fake', '.', timeout); cleanup.push(() => pool.close());
  const creating = pool.create(); await flush(); const child = children.at(-1)!; child.reply(child.lines()[0].id);
  const { id } = await creating;
  const running = pool.call(id, { method: 'run' }); const waiting = pool.call(id, { method: 'status' });
  await flush(); const firstId = child.lines()[0].id;
  await vi.advanceTimersByTimeAsync(16 * 60_000); expect(child.kill).not.toHaveBeenCalled();
  child.reply(firstId); await running; await flush(); const nextId = child.lines()[0].id;
  await vi.advanceTimersByTimeAsync(16 * 60_000); expect(child.kill).not.toHaveBeenCalled();
  child.reply(nextId); await waiting;
  await vi.advanceTimersByTimeAsync(15 * 60_000); expect(child.kill).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(60_000); expect(child.kill).toHaveBeenCalledOnce();
  await expect(pool.call(id, { method: 'status' })).rejects.toMatchObject({ code: 'session-lost' });
});

it.each(['60000', 'unlimited'])('uses host configuration %s for single-shot CLI calls', async value => {
  vi.stubEnv(RESPONSE_TIMEOUT_ENV, value);
  const result = outcome(invokeCore('list-nodes', '')); const child = children.at(-1)!;
  await vi.advanceTimersByTimeAsync(value === 'unlimited' ? 24 * 60 * 60_000 : 59_999);
  expect(child.kill).not.toHaveBeenCalled();
  if (value === 'unlimited') { child.stdout.emit('data', Buffer.from('[]')); child.emit('close', 0); expect((await result).value).toEqual([]); }
  else { await vi.advanceTimersByTimeAsync(1); expect((await result).error).toMatchObject({ code: 'timeout' }); expect(child.kill).toHaveBeenCalledOnce(); }
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['60000', 'unlimited'])('reads host setting %s once and forwards it to sessions and CLI calls', async value => {
  vi.stubEnv(RESPONSE_TIMEOUT_ENV, value);
  const middleware = createApiMiddleware(); cleanup.push(middleware.close);
  // 起動後に環境が変わっても、ホストは起動時の設定を使う。
  vi.stubEnv(RESPONSE_TIMEOUT_ENV, 'invalid');
  const request = { url: '/api/sessions', method: 'POST', headers: { host: '127.0.0.1:5173' } } as import('node:http').IncomingMessage;
  const response = Object.assign(new EventEmitter(), { destroyed: false, writableEnded: false, writeHead: vi.fn(), end: vi.fn() });
  middleware(request, response as unknown as import('node:http').ServerResponse, vi.fn());
  await flush(); const child = children.at(-1)!;
  expect(child.lines()).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(value === 'unlimited' ? 16 * 60_000 : 59_999);
  expect(child.kill).not.toHaveBeenCalled();
  if (value === 'unlimited') { child.reply(1); await flush(); expect(response.writeHead).toHaveBeenCalledWith(200, expect.anything()); }
  else { await vi.advanceTimersByTimeAsync(1); expect(response.writeHead).toHaveBeenCalledWith(503, expect.anything()); expect(child.kill).toHaveBeenCalledOnce(); }
  const cliResponse = Object.assign(new EventEmitter(), { destroyed: false, writableEnded: false, writeHead: vi.fn(), end: vi.fn() });
  middleware({ ...request, url: '/api/nodes', method: 'GET' } as import('node:http').IncomingMessage, cliResponse as unknown as import('node:http').ServerResponse, vi.fn());
  const cli = children.at(-1)!; expect(cli).not.toBe(child);
  await vi.advanceTimersByTimeAsync(10_001); expect(cli.kill).not.toHaveBeenCalled();
  cli.stdout.emit('data', Buffer.from('[]')); cli.emit('close', 0); await flush();
  expect(cliResponse.writeHead).toHaveBeenCalledWith(200, expect.anything());
});

it('does not retain or reflect raw persistent Core stderr in failure replies', async () => {
  const { core, child } = worker(null);
  const result = outcome(core.request({ method: 'run' }));
  await flush();
  child.stderr.emit('data', Buffer.from('input-and-key-canary\n'));
  child.emit('close', 1);
  const error = (await result).error;
  expect(error).toMatchObject({ code: 'session-lost' });
  expect(error.message).not.toContain('input-and-key-canary');
  expect(Object.values(core).some(value => typeof value === 'string' && value.includes('input-and-key-canary'))).toBe(false);
});

it('does not retain or reflect raw v3 broker stderr in failure replies', async () => {
  const broker = new ExecutionBroker({ core: 'fake', root: '.', policyFile: 'fake-policy', responseTimeoutMs: null });
  const child = children.at(-1)!;
  const result = outcome(broker.request({ method: 'open' }));
  child.stderr.emit('data', Buffer.from('v3-input-and-key-canary\n'));
  child.emit('close', 1, null);
  const error = (await result).error;
  expect(error).toMatchObject({ code: 'host-lost' });
  expect(error.message).not.toContain('v3-input-and-key-canary');
  expect(Object.values(broker).some(value => typeof value === 'string' && value.includes('v3-input-and-key-canary'))).toBe(false);
  await broker.exited;
});
it('documents raw single-shot diagnostics while keeping concurrent and later calls isolated', async () => {
  const first=outcome(invokeCore('validate-graph','{}',undefined,null));const a=children.at(-1)!;
  const second=outcome(invokeCore('validate-graph','{}',undefined,null));const b=children.at(-1)!;
  a.stderr.emit('data',Buffer.from('unknown-kind: single-shot-input-key-canary\n'));
  b.stderr.emit('data',Buffer.from('invalid-config: independent-error\n'));
  a.emit('close',1);b.emit('close',1);
  expect((await first).error.message).toContain('single-shot-input-key-canary');
  expect((await second).error.message).not.toContain('single-shot-input-key-canary');
  const third=outcome(invokeCore('validate-graph','{}',undefined,null));const c=children.at(-1)!;c.emit('close',1);
  expect((await third).error.message).not.toContain('single-shot-input-key-canary');
});
it('does not copy raw single-shot stderr into valid success or structured run reports', async () => {
  for(const [command,code,data]of [['list-nodes',0,[]],['run',1,{succeeded:false,nodes:{probe:{diagnostic:{error:{code:'fake',message:'structured-canary'}}}}}]] as const){
    const result=outcome(invokeCore(command,'{}',undefined,null));const child=children.at(-1)!;
    child.stderr.emit('data',Buffer.from('raw-stderr-only-canary\n'));child.stdout.emit('data',Buffer.from(JSON.stringify(data)));child.emit('close',code);
    expect((await result).value).toEqual(data);expect(JSON.stringify((await result).value)).not.toContain('raw-stderr-only-canary');
  }
});
