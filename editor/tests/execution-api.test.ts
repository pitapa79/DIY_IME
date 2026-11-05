import { expect, it, vi } from 'vitest';
import { createServer } from 'node:http';
import { basename, dirname, join, resolve } from 'node:path';
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createApiMiddleware } from '../server/bridge.ts';
import { createExecutionApi, ExecutionApiError } from '../src/executionApi';
import { acceptExecution, beginExecution, createExecutionView, executionDataVisible, executionIntent, observeExecution, loseExecution, recoverExecution, updateExecutionContext } from '../src/executionView';
import type { ExecutionSnapshot, ExecutionWireState } from '../src/executionModel';
const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)), '..');
const CORE = resolve(ROOT, 'target/debug', process.platform === 'win32' ? 'diy-ime2.exe' : 'diy-ime2');
const epoch = '0123456789abcdef0123456789abcdef';
const wire: ExecutionWireState = { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' };
const policy = JSON.parse(readFileSync(resolve(ROOT, 'tests/contracts/execution_control.json'), 'utf8')).resourceProfiles.bounded;
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const makeFetch = (body: unknown, status = 200) => vi.fn<typeof fetch>().mockResolvedValue(json(body, status));
const start = { startKey: '18446744073709551615', expectedPlanVersion: '9007199254740993', targets: [{ node: 'source', port: 'out' }] };
it('sends canonical u64 strings once with the explicit v3 paths and preserves omitted budgets', async () => {
  const key = { sessionEpoch: epoch, planVersion: start.expectedPlanVersion, runId: '18446744073709551615', deliveryGeneration: '1' };
  const fetcher = makeFetch({ ok: true, data: { state: wire, result: { state: 'accepted', replayed: false, key } } });
  const reply = await createExecutionApi(fetcher).start(epoch, start);
  expect(reply.result.key).toEqual(key);expect(fetcher).toHaveBeenCalledTimes(1);
  const [path, init] = fetcher.mock.calls[0];expect(path).toBe(`/api/v3/sessions/${epoch}/request`);
  expect(JSON.parse(init!.body as string)).toEqual({ ...start, method: 'start-run' });
  expect(JSON.parse(init!.body as string)).not.toHaveProperty('runBudgetMs');
});
it('distinguishes known Core refusal from connection failure without retrying a start', async () => {
  const refusal = makeFetch({ ok: false, error: { code: 'busy', message: 'full' }, state: wire }, 503);
  await expect(createExecutionApi(refusal).start(epoch, start)).rejects.toMatchObject({ code: 'busy', state: wire, resultUnknown: false });expect(refusal).toHaveBeenCalledTimes(1);
  for (const fetcher of [vi.fn<typeof fetch>().mockRejectedValue(new Error('lost')), vi.fn<typeof fetch>().mockRejectedValue(new DOMException('aborted after send', 'AbortError')), makeFetch({ ok: false, error: { code: 'session-lost', message: 'lost' } }, 503)]) {
    await expect(createExecutionApi(fetcher).start(epoch, start)).rejects.toMatchObject({ resultUnknown: true });expect(fetcher).toHaveBeenCalledTimes(1);
  }
});
it('rejects a success for another epoch, plan or full requested run key', async () => {
  const key = { sessionEpoch: epoch, planVersion: start.expectedPlanVersion, runId: '1', deliveryGeneration: '1' };
  for (const edit of [{ sessionEpoch: 'fedcba9876543210fedcba9876543210' }, { planVersion: '1' }, { inputRevision: 1 }]) {
    const fetcher = makeFetch({ ok: true, data: { state: wire, result: { state: 'accepted', replayed: false, key: { ...key, ...edit } } } });
    await expect(createExecutionApi(fetcher).start(epoch, start)).rejects.toMatchObject({ code: 'invalid-response', resultUnknown: true });expect(fetcher).toHaveBeenCalledTimes(1);
  }
  const wrong = { key: { ...key, runId: '2' }, state: 'running', eventSeq: '1', elapsedMs: 0, current: true, physical: { running: 1, cancelRequested: 0, stalled: 0, queued: 0 }, pollAfterMs: 1 };
  const fetcher = makeFetch({ ok: true, data: { state: wire, result: wrong } });await expect(createExecutionApi(fetcher).getRun(epoch, key)).rejects.toMatchObject({ code: 'invalid-response', resultUnknown: true });expect(fetcher).toHaveBeenCalledTimes(1);
});
it('does not send a pre-aborted request or silently convert invalid budgets to unlimited', async () => {
  const fetcher = vi.fn<typeof fetch>();const api = createExecutionApi(fetcher);const abort = new AbortController();abort.abort();
  await expect(api.open(abort.signal)).rejects.toMatchObject({ code: 'aborted', resultUnknown: false });
  for (const runBudgetMs of [0, -1, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1]) expect(() => api.start(epoch, { ...start, runBudgetMs })).toThrow(ExecutionApiError);
  expect(() => api.start(epoch, { ...start, startKey: '18446744073709551616' })).toThrow(ExecutionApiError);
  expect(fetcher).not.toHaveBeenCalled();
});
it('marks malformed JSON, legacy state, contradictory outcomes and unsafe counters as unknown', async () => {
  const valid = { id: epoch, state: wire, resourceDomain: 'test', resourcePolicy: policy };
  for (const value of [{ ok: true, data: { ...valid, state: { ...wire, revision: 0 } } }, { ok: true, data: valid, error: { code: 'bad', message: 'bad' } },
    { ok: true, data: { ...valid, id: 'old-session' } },
    { ok: true, data: { ...valid, resourcePolicy: { ...policy, maxQueuedInputBytes: 0 } } },
    { ok: true, data: { ...valid, resourcePolicy: { ...policy, queueWaitMs: 0 } } },
    { ok: true, data: { ...valid, resourcePolicy: { ...policy, maxResultBytesPerRun: policy.maxRetainedBytes + 1 } } }, { ok: false, error: { code: 'busy', message: 'bad' }, state: { ...wire, deliveryGeneration: '01' } }]) {
    const fetcher = makeFetch(value, value.ok ? 200 : 503);await expect(createExecutionApi(fetcher).open()).rejects.toMatchObject({ code: 'invalid-response', resultUnknown: true });expect(fetcher).toHaveBeenCalledTimes(1);
  }
  const badJson = vi.fn<typeof fetch>().mockResolvedValue(new Response('{'));await expect(createExecutionApi(badJson).open()).rejects.toMatchObject({ resultUnknown: true });expect(badJson).toHaveBeenCalledTimes(1);
});
async function until(check: () => boolean) { const end = Date.now() + 5000; while (!check()) { if (Date.now() >= end) throw new Error('test Host did not drain');await new Promise(resolve => setTimeout(resolve, 2)); } }
function cleanup(directory: string) {
  const actual = realpathSync(directory), base = realpathSync(tmpdir());
  if (dirname(actual) !== base || !basename(actual).startsWith('diy-ime-v3-client-')) throw new Error('unexpected test directory');
  rmSync(actual, { recursive: true, force: true });
}
it('uses the typed client with the real HTTP/Core, adopts only the current run and refuses an old epoch', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'diy-ime-v3-client-')), policyFile = join(directory, 'policy.json');writeFileSync(policyFile, JSON.stringify(policy));
  const middleware = createApiMiddleware({ execution: { policyFile, core: CORE, root: ROOT, responseTimeoutMs: 5000 } });
  const server = createServer((request, response) => middleware(request, response, () => { response.statusCode = 404;response.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const address = server.address();if (!address || typeof address === 'string') throw new Error('listen');
  const api = createExecutionApi((path, init) => fetch(new URL(String(path), `http://127.0.0.1:${address.port}`), init));
  const graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }], edges: [] };
  try {
    const session = await api.open();expect(session.state.activePlanVersion).toBeNull();expect(session.resourcePolicy).toEqual(policy);
    expect((await api.definitions(session.id)).result.some(item => item.version === 2 && item.definition.kind === 'input.text')).toBe(true);
    expect((await api.validate(session.id, graph)).result).toEqual({ valid: true, order: ['source'] });
    const prepared = await api.prepare(session.id, graph), active = await api.activate(session.id, prepared.result.token);
    expect((await api.status(session.id)).state).toEqual(active.state);
    const input = { revision: 1, text: 'かな😀', mode: 'compose' as const };
    const context = { sessionEpoch: session.id, state: active.state, input };const intent = executionIntent(context);
    let view = beginExecution(createExecutionView(context), intent);
    const receipt = await api.start(session.id, { startKey: '9007199254740993', expectedPlanVersion: active.state.activePlanVersion!, targets: [{ node: 'source', port: 'out' }], input, runBudgetMs: null, jobBudgets: [{ nodeId: 'source', budgetMs: null }] });
    expect(receipt.result.replayed).toBe(false);view = updateExecutionContext(view, { ...context, state: receipt.state });view = acceptExecution(view, intent, receipt.result);expect(view.phase).toBe('accepted');
    expect((await api.getStart(session.id, '9007199254740993')).result.key).toEqual(receipt.result.key);
    let snapshot: ExecutionSnapshot;const deadline = Date.now() + 5000;
    do { snapshot = (await api.getRun(session.id, receipt.result.key)).result;if (snapshot.state === 'terminal') break;if (Date.now() > deadline) throw new Error('run timeout');await new Promise(resolve => setTimeout(resolve, snapshot.pollAfterMs)); } while (true);
    view = observeExecution(view, intent, snapshot);expect(executionDataVisible(view)).toBe(true);
    expect(snapshot.result!.ports[0].value).toEqual({ status: 'data', packet: { version: 1, root: { type: 'text', text: 'かな😀' } } });
    expect((await api.cancel(session.id, receipt.result.key)).result.result).toEqual(snapshot.result);
    const next = await api.prepare(session.id, graph);expect(next.state.activePlanVersion).toBe('1');expect(next.state.preparedToken).not.toBeNull();
    const activated = await api.activate(session.id, next.result.token);expect(activated.state.activePlanVersion).toBe('2');
    expect(updateExecutionContext(view, { ...context, state: activated.state }).phase).toBe('idle');
    const rolled = await api.rollback(session.id, '2');expect(rolled.state.activePlanVersion).toBe('3');
    const invalidated = await api.invalidate(session.id, '3', 2);expect(invalidated.state.latestInputRevision).toBe(2);
    // Lose an actual accepted response body without closing the Session or
    // reading its key; an explicit get-start is the only recovery request.
    let startCalls = 0;
    const lostApi = createExecutionApi(async (path, init) => {
      const response = await fetch(new URL(String(path), `http://127.0.0.1:${address.port}`), init);
      if (init?.body && JSON.parse(String(init.body)).method === 'start-run') {
        startCalls++;await response.arrayBuffer();return new Response('{');
      }
      return response;
    });
    const secondContext = { sessionEpoch: session.id, state: invalidated.state, input: { ...input, revision: 2, text: '次の入力' } };
    const secondIntent = executionIntent(secondContext);let uncertain = beginExecution(createExecutionView(secondContext), secondIntent);
    await expect(lostApi.start(session.id, { startKey: '9007199254740994', expectedPlanVersion: '3', targets: [{ node: 'source', port: 'out' }], input: secondContext.input })).rejects.toMatchObject({ resultUnknown: true });
    uncertain = loseExecution(uncertain, secondIntent);const found = await lostApi.getStart(session.id, '9007199254740994');
    uncertain = updateExecutionContext(uncertain, { ...secondContext, state: found.state });uncertain = recoverExecution(uncertain, secondIntent, found.result.key);
    expect(uncertain.phase).toBe('accepted');expect(executionDataVisible(uncertain)).toBe(false);expect(found.result.key.inputRevision).toBe(2);expect(startCalls).toBe(1);
    await api.close(session.id);const reopened = await api.open();expect(reopened.id).not.toBe(session.id);expect(reopened.resourceDomain).toBe(session.resourceDomain);
    await expect(api.getRun(reopened.id, receipt.result.key)).rejects.toMatchObject({ code: 'session-conflict', resultUnknown: false });await api.close(reopened.id);
  } finally {
    middleware.close();server.closeAllConnections();await new Promise<void>(resolve => server.close(() => resolve()));await until(() => !existsSync(policyFile + '.broker-lock'));cleanup(directory);
  }
});
