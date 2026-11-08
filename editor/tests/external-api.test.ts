import { expect, it, vi } from 'vitest';
import { createExternalApi, ExternalApiError } from '../src/externalApi';
const id = 'a'.repeat(32), epoch = 'b'.repeat(32), reviewId = 'c'.repeat(32);
const state = { phase: 'ready', protected: false, sessionEpoch: epoch, reviewId: null, state: { revision: '0', activePlanVersion: '1', preparedToken: null, canRollback: false, latestInputRevision: 2, deliveryGeneration: '2' }, physical: { physicalJobs: 0 }, workerProcesses: 0 };
const key = { sessionEpoch: epoch, planVersion: '1', runId: '1', inputRevision: 1, deliveryGeneration: '1' };
const fetcher = (data: unknown) => vi.fn(async () => new Response(JSON.stringify({ ok: true, data }), { status: 200 }));
it('separates retained native ownership from callback usage and rejects malformed observations', async () => {
  const retainedWorker = {physicalJobs:1,ready:true,processId:123,generation:1,activeInput:false,decodedReplies:4};
  const withWorker = {...state,physical:{physicalJobs:1},workerProcesses:1,retainedWorker};
  expect((await createExternalApi(fetcher(withWorker)).status(id)).retainedWorker).toEqual(retainedWorker);
  expect((await createExternalApi(fetcher(state)).status(id)).retainedWorker).toBeUndefined();
  for(const invalid of [{...retainedWorker,physicalJobs:2},{...retainedWorker,processId:0},{...retainedWorker,processId:0x100000000},{...retainedWorker,generation:-1},{...retainedWorker,ready:'true'},{...retainedWorker,activeInput:null},{...retainedWorker,trusted:true}]) {
    await expect(createExternalApi(fetcher({...withWorker,retainedWorker:invalid})).status(id)).rejects.toMatchObject({code:'external-response'});
  }
});
it('normalizes only explicit withdrawn stale payloads and refuses a current null result', async () => {
  const snapshot = { key, state: 'terminal', eventSeq: '3', elapsedMs: 5, current: false, physical: { running: 0, cancelRequested: 0, stalled: 0, queued: 0 }, pollAfterMs: 50, result: null };
  expect((await createExternalApi(fetcher({ snapshot, state })).getRun(id, key)).snapshot.result).toBeUndefined();
  await expect(createExternalApi(fetcher({ snapshot: { ...snapshot, current: true }, state })).getRun(id, key)).rejects.toBeInstanceOf(ExternalApiError);
  await expect(createExternalApi(fetcher({ snapshot: { ...snapshot, key: { ...key, runId: '2' } }, state })).getRun(id, key)).rejects.toBeInstanceOf(ExternalApiError);
});
it('refuses unknown review fields, forged missing subjects and duplicate subjects', async () => {
  const request = { id: 'd'.repeat(64), kind: 'execute', packageId: 'test', nodes: ['worker'], source: null };
  for (const data of [{ reviewId, sessionEpoch: epoch, requests: [request], missing: [], trusted: true }, { reviewId, sessionEpoch: epoch, requests: [request], missing: [{ ...request, packageId: 'other' }] }, { reviewId, sessionEpoch: epoch, requests: [request, request], missing: [] }]) {
    await expect(createExternalApi(fetcher(data)).prepare(id, { version: 1, nodes: [], edges: [] })).rejects.toMatchObject({ code: 'external-response' });
  }
});
it('treats lost start responses as unknown and sends only one start request', async () => {
  const fetch = vi.fn(async () => { throw Error('lost'); }), api = createExternalApi(fetch);
  await expect(api.start(id, { startKey: '1', expectedPlanVersion: '1', targets: [] })).rejects.toMatchObject({ unknownResult: true });expect(fetch).toHaveBeenCalledTimes(1);
});
it('keeps known Host denials distinct from unavailable or invalid responses', async () => {
  const denied = createExternalApi(vi.fn(async () => new Response(JSON.stringify({ ok: false, error: { code: 'permission-denied', message: 'denied' } }), { status: 422 })));
  await expect(denied.status(id)).rejects.toMatchObject({ code: 'permission-denied', unknownResult: false });
  const failed = createExternalApi(vi.fn(async () => new Response(JSON.stringify({ ok: false, error: { code: 'ended', message: 'ended' } }), { status: 503 })));
  await expect(failed.status(id)).rejects.toMatchObject({ unknownResult: true });
});
it('accepts only a distinct resource-read description and refuses forged extra root/authority fields', async () => {
  const resource = { id: 'f'.repeat(64), kind: 'resourceRead', packageId: 'test', nodes: ['worker'], source: null, resource: { binding: 'dictionary', reference: 'sample.tsv', policySha256: '0'.repeat(64) } };
  const reply = (request: unknown) => ({ reviewId, sessionEpoch: epoch, requests: [request], missing: [request] });
  const api = createExternalApi(fetcher(reply(resource)));expect((await api.prepare(id, { version: 1, nodes: [], edges: [] })).missing[0].kind).toBe('resourceRead');
  for (const value of [{ ...resource, resource: { ...resource.resource, root: 'private-root' } }, { ...resource, kind: 'execute' }, { ...resource, resource: { ...resource.resource, policySha256: 'fake' } }, { ...resource, source: { node: 'fake', port: 'out' } }]) {
    await expect(createExternalApi(fetcher(reply(value))).prepare(id, { version: 1, nodes: [], edges: [] })).rejects.toMatchObject({ code: 'external-response' });
  }
});
it('accepts only secret metadata and refuses plaintext/ciphertext/duplicate exports and a mismatched put identity', async () => {
  const entry = { id: 'service', version: 'f'.repeat(32) };
  expect(await createExternalApi(fetcher({ available: true, entries: [entry] })).secrets(id)).toEqual({ available: true, entries: [entry] });
  for (const data of [{ available: true, entries: [{ ...entry, value: 'private' }] }, { available: true, entries: [{ ...entry, ciphertext: [1] }] }, { available: true, entries: [entry, entry] }, { available: false, entries: [entry] }, { available: true, entries: [{ ...entry, version: 'private' }] }]) await expect(createExternalApi(fetcher(data)).secrets(id)).rejects.toMatchObject({ code: 'external-response' });
  await expect(createExternalApi(fetcher({ entry: { ...entry, id: 'other' }, state })).secretPut(id, 'service', 'private')).rejects.toMatchObject({ code: 'external-response' });
});
it('keeps network and secret consent distinct and refuses private fields, mixed scopes and invalid bounds', async () => {
  const network = { binding: 'api', method: 'POST', destination: 'https://service.example/fixed', policySha256: '0'.repeat(64), limits: { maxCalls: 2, maxRequestBytes: 4096, maxResponseBytes: 8192, maxContinuationBytes: 1024, budgetMs: 30000 }, secret: { id: 'service', version: 'f'.repeat(32) } };
  const request = { id: 'd'.repeat(64), kind: 'networkUse', packageId: 'test', nodes: ['worker'], source: null, network };
  const reply = (request: unknown) => ({ reviewId, sessionEpoch: epoch, requests: [request], missing: [request] });
  for (const kind of ['networkUse', 'secretUse']) expect((await createExternalApi(fetcher(reply({ ...request, kind }))).prepare(id, { version: 1, nodes: [], edges: [] })).missing[0].kind).toBe(kind);
  for (const bad of [
    { ...request, network: { ...network, headers: { Authorization: 'private' } } },
    { ...request, network: { ...network, secret: { ...network.secret, value: 'private' } } },
    { ...request, network: { ...network, secret: { ...network.secret, ciphertext: [1] } } },
    { ...request, network: { ...network, limits: { ...network.limits, maxCalls: 0 } } },
    { ...request, network: { ...network, destination: 'https://private@service.example/fixed' } },
    { ...request, kind: 'execute' }, { ...request, source: { node: 'other', port: 'out' } },
    { ...request, resource: { binding: 'dictionary', reference: 'file', policySha256: '0'.repeat(64) } },
    { ...request, kind: 'secretUse', network: { ...network, secret: undefined } },
  ]) await expect(createExternalApi(fetcher(reply(bad))).prepare(id, { version: 1, nodes: [], edges: [] })).rejects.toMatchObject({ code: 'external-response' });
});
