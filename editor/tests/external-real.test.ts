import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
import { createExternalApi, ExternalApiError } from '../src/externalApi';
import type { Graph } from '../src/model';
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
it('executes a consented installed package through real HTTP/client/Host/AppContainer and drains protection and disconnect', async () => {
  // Fixed test milestones only: no input, response, paths, IDs or guest messages.
  console.info('external-real: package-import');
  const folder = mkdtempSync(join(tmpdir(), 'diyime-external-http-real-')), bin = join(ROOT, 'target/debug');
  const wasm = join(ROOT, 'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm');
  const worker = join(bin, 'diy-ime2-wasm-worker.exe');
  const dll = process.env.DIYIME2_TEST_WASMTIME_DLL ?? join(ROOT, 'target/tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll');
  const dataRoot = join(folder, 'data'), config = join(folder, 'config.json'), manifest = join(folder, 'manifest.json'), packed = join(folder, 'fixture.diynode');
  const definition = { kind: 'fixture.wasm', version: 1, inputs: [{ port: { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' }, required: true }], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } };
  writeFileSync(manifest, JSON.stringify({ packageId: 'test.http', version: '1.0.0', displayName: 'HTTP fixture', runtime: 'wasm32', guestApiVersion: 1, node: definition, moduleSha256: createHash('sha256').update(readFileSync(wasm)).digest('hex'), capabilities: [] }));
  function command(args: string[]) { const r = spawnSync(join(bin, 'diy-ime2-node-package.exe'), args, { encoding: 'utf8', windowsHide: true });expect(r.status, r.stderr).toBe(0); }
  command(['pack', manifest, wasm, packed]);command(['import', dataRoot, join(folder, 'install'), worker, dll, packed, '--commit']);
  const resourcePolicy = JSON.parse(readFileSync(join(ROOT, 'tests/contracts/execution_control.json'), 'utf8')).resourceProfiles.bounded;
  Object.assign(resourcePolicy, { maxPhysicalJobs: 1, maxQueuedJobs: 0, retentionMs: 60000 });
  writeFileSync(config, JSON.stringify({ packageManager: { dataRoot, workspace: join(folder, 'workers'), worker, runtimeDll: dll, policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: null, walltimeMs: null, terminationGraceMs: 5 } }, permissionRoot: join(folder, 'permissions'), resourcePolicy, requestBytes: 1048576, responseBytes: 16777216 }));
  const middleware = createApiMiddleware({ external: { executable: join(bin, 'diy-ime2-external-host.exe'), config } });
  const server = createServer((req, res) => middleware(req, res, () => { res.statusCode = 404;res.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const address = server.address();if (!address || typeof address === 'string') throw Error('listen');
  const origin = 'http://127.0.0.1:' + address.port, api = createExternalApi((path, init) => fetch(origin + path, init));
  const graph = (mode: string): Graph => ({ version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }, { id: 'worker', kind: 'fixture.wasm', version: 1, config: { mode } }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'worker', port: 'in' } }] });
  async function until<T>(check: () => Promise<T | null>): Promise<T> { const deadline = Date.now() + 60000;for (;;) { const value = await check();if (value) return value;if (Date.now() > deadline) throw Error('HTTP Host did not settle');await new Promise(resolve => setTimeout(resolve, 30)); } }
  let id: string | undefined;
  try {
    console.info('external-real: open-and-review');
    const opened = await api.open();id = opened.connectionId;expect(opened.status.phase).toBe('empty');expect((await api.definitions(id)).definitions).toHaveLength(5);
    const review = await api.prepare(id, graph('echo'));expect(review.missing).toHaveLength(2);await expect(api.activate(id, review.reviewId)).rejects.toBeInstanceOf(ExternalApiError);
    const retry = await api.prepare(id, graph('echo'));await api.approve(id, retry, retry.missing.map(r => r.id), true);
    // A denied activation consumes that review; a new preparation can reuse the saved subjects.
    const approved = await api.prepare(id, graph('echo'));expect(approved.missing).toHaveLength(0);const active = await api.activate(id, approved.reviewId);expect(active.phase).toBe('ready');
    const start = { startKey: '1', expectedPlanVersion: '1', targets: [{ node: 'worker', port: 'out' }], input: { revision: 1, text: 'private-http-input', mode: 'compose' as const } };
    console.info('external-real: normal-detailed-run');
    await api.traceStartDetailed(id);
    const receipt = await api.start(id, start);expect((await api.getStart(id, '1')).key).toEqual(receipt.key);
    const result = await until(async () => { const r = await api.getRun(id!, receipt.key);return r.snapshot.state === 'terminal' ? r.snapshot : null; });expect(JSON.stringify(result.result)).toContain('private-http-input');
    await until(async()=>{const s=await api.status(id!);return s.physical.physicalJobs===0?s:null;});
    const detailed=await api.traceRead(id,'0',256), kinds=detailed.events.map(e=>e.event);
    for(const phase of ['host-request','worker-staging','worker-process','worker-cleanup']){
      const begin=kinds.indexOf((phase+'-started') as typeof kinds[number]),end=kinds.indexOf((phase+'-finished') as typeof kinds[number]);
      expect(begin>=0||detailed.dropped>0).toBe(true);expect(end>=0||detailed.dropped>0).toBe(true);
      if(begin>=0&&end>=0)expect(begin).toBeLessThan(end);
    }
    expect(JSON.stringify(detailed)).not.toContain('private-http-input');expect(JSON.stringify(detailed)).not.toContain(receipt.key.sessionEpoch);
    const workerIntervals=detailed.events.flatMap(e=>e.worker?[e.worker]:[]);
    for(const phase of ['main','dll-load','module-read','input-read','engine-setup','compile','instantiate','guest-setup','guest-call','reply-read','reply-write','cleanup']){
      expect(workerIntervals.some(w=>w.phase===phase&&w.instance===1&&w.finishedUs!==undefined)||detailed.dropped>0).toBe(true);
    }
    console.info('external-real: invalidate');
    await api.invalidate(id, '1', 2);const stale = await api.getRun(id, receipt.key);expect(stale.snapshot.current).toBe(false);expect(stale.snapshot.result).toBeUndefined();
    console.info('external-real: prepare-hold');
    const holding = await api.prepare(id, graph('hold'));await api.approve(id, holding, holding.missing.map(r => r.id), false);await api.activate(id, holding.reviewId);
    console.info('external-real: cancel-start');
    await api.traceStartDetailed(id);const cancelled=await api.start(id,start);
    await until(async()=>{const s=await api.status(id!);return s.workerProcesses>0?s:null;});
    await new Promise(resolve=>setTimeout(resolve,300));
    console.info('external-real: cancel-request');
    const waiting = api.getRun(id, cancelled.key);
    await api.cancelRun(id,cancelled.key);
    expect(JSON.stringify((await waiting).snapshot.result) ?? '').not.toContain('private-http-input');
    console.info('external-real: cancel-drain');
    await until(async()=>{const s=await api.status(id!);return s.workerProcesses===0&&s.physical.physicalJobs===0?s:null;});
    const partial=await api.traceRead(id,'0',256),partialIntervals=partial.events.flatMap(e=>e.worker?[e.worker]:[]);
    expect(partialIntervals.length>0||partial.dropped>0).toBe(true);
    for(const w of partialIntervals.filter(w=>w.phase==='main'||w.phase==='guest-call'))expect(w.finishedUs).toBeUndefined();
    console.info('external-real: protection-start');
    await api.start(id, {...start,startKey:'2'});await until(async () => { const s = await api.status(id!);return s.workerProcesses > 0 ? s : null; });
    console.info('external-real: protection-request');
    await api.protect(id, true);
    console.info('external-real: protection-drain');
    const stopped = await until(async () => { const s = await api.status(id!);return s.workerProcesses === 0 && s.physical.physicalJobs === 0 ? s : null; });expect(stopped.phase).toBe('protected');
    console.info('external-real: restore-and-revoke');
    await api.protect(id, false);const restored = await api.prepare(id, graph('echo'));expect(restored.missing).toHaveLength(0);await api.activate(id, restored.reviewId);
    await api.revoke(id, restored.requests.find(r => r.kind === 'execute')!.id);expect((await api.status(id)).phase).toBe('empty');
    console.info('external-real: close-first');
    await api.close(id);const old = id;id = undefined;
    console.info('external-real: reopen');
    const next = await api.open();id = next.connectionId;expect(id).not.toBe(old);await api.close(old);expect((await api.status(id)).phase).toBe('empty');
    console.info('external-real: disconnect-start');
    const pending = await api.prepare(id, graph('hold'));await api.approve(id, pending, pending.missing.map(r => r.id), false);await api.activate(id, pending.reviewId);await api.start(id, start);
    await until(async () => { const s = await api.status(id!);return s.workerProcesses > 0 ? s : null; });
    console.info('external-real: disconnect-close');
    await api.close(id);id = undefined;
    expect(readFileSync(join(folder, 'permissions/permissions.json'), 'utf8')).not.toContain('private-http-input');
  } finally {
    console.info('external-real: cleanup-connection');
    if (id) await api.close(id);
    console.info('external-real: cleanup-middleware');
    await middleware.close();
    console.info('external-real: cleanup-server');
    server.closeAllConnections();await new Promise<void>(resolve => server.close(() => resolve()));
    console.info('external-real: cleanup-files');
    if (dirname(folder) !== resolve(tmpdir()) || !folder.startsWith(join(tmpdir(), 'diyime-external-http-real-'))) throw Error('unexpected fixture cleanup scope');rmSync(folder, { recursive: true });
  }
}, 180000);
it('imports and executes an explicitly consented dictionary snapshot through real HTTP and withdraws its read grant', async () => {
  const folder = mkdtempSync(join(tmpdir(), 'diyime-dictionary-http-real-')), bin = join(ROOT, 'target/debug');
  const wasm = join(ROOT, 'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm');
  const worker = join(bin, 'diy-ime2-wasm-worker.exe');
  const dll = process.env.DIYIME2_TEST_WASMTIME_DLL ?? join(ROOT, 'target/tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll');
  const resources = join(folder, 'resources');mkdirSync(resources);
  const dictionary = join(resources, 'echo'), first = 'http-dictionary-private', input = 'http-runtime-private';
  const managerConfig = { dataRoot: join(folder, 'data'), workspace: join(folder, 'workers'), worker, runtimeDll: dll, policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: null, walltimeMs: null, terminationGraceMs: 5 }, dictionaryBindings: { dictionary: { root: resources, referencePointer: '/mode', maxFileBytes: 128, maxTotalBytes: 256, maxFiles: 4, budgetMs: 5000 } } };
  const managerPath = join(folder, 'manager.json'), config = join(folder, 'host.json'), manifest = join(folder, 'manifest.json'), packed = join(folder, 'fixture.diynode');
  const resourcePolicy = JSON.parse(readFileSync(join(ROOT, 'tests/contracts/execution_control.json'), 'utf8')).resourceProfiles.bounded;
  Object.assign(resourcePolicy, { maxPhysicalJobs: 2, maxQueuedJobs: 4, retentionMs: 60000 });
  writeFileSync(managerPath, JSON.stringify({ ...managerConfig, dictionaryBindings: {} }));writeFileSync(config, JSON.stringify({ packageManager: managerConfig, permissionRoot: join(folder, 'permissions'), resourcePolicy, requestBytes: 1048576, responseBytes: 16777216 }));
  const definition = { kind: 'fixture.wasm', version: 1, inputs: [{ port: { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' }, required: true }], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } };
  writeFileSync(manifest, JSON.stringify({ packageId: 'test.dictionary.http', version: '1.0.0', displayName: 'Dictionary HTTP fixture', runtime: 'wasm32', guestApiVersion: 1, node: definition, moduleSha256: createHash('sha256').update(readFileSync(wasm)).digest('hex'), capabilities: [{ id: 'dictionary.read', binding: 'dictionary' }] }));
  const packedResult = spawnSync(join(bin, 'diy-ime2-node-package.exe'), ['pack', manifest, wasm, packed], { encoding: 'utf8', windowsHide: true });expect(packedResult.status, packedResult.stderr).toBe(0);
  const middleware = createApiMiddleware({ packages: { executable: join(bin, 'diy-ime2-node-manager.exe'), config: managerPath }, external: { executable: join(bin, 'diy-ime2-external-host.exe'), config } });
  const server = createServer((req, res) => middleware(req, res, () => { res.statusCode = 404;res.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const address = server.address();if (!address || typeof address === 'string') throw Error('listen');
  const origin = 'http://127.0.0.1:' + address.port, api = createExternalApi((path, init) => fetch(origin + path, init));
  async function post(path: string, body: Buffer | string, type: string) { const response = await fetch(origin + path, { method: 'POST', headers: { 'Content-Type': type }, body: typeof body === 'string' ? body : new Uint8Array(body) });const value = await response.json();expect(response.status, JSON.stringify(value)).toBe(200);return value.data; }
  const graph: Graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }, { id: 'dictionary', kind: 'fixture.wasm', version: 1, config: { mode: 'echo' } }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'dictionary', port: 'in' } }] };
  let id: string | undefined;
  async function result(startKey: string) {
    const receipt = await api.start(id!, { startKey, expectedPlanVersion: '1', targets: [{ node: 'dictionary', port: 'out' }], input: { revision: 1, text: input, mode: 'compose' } });
    const end = Date.now() + 60000;
    for (;;) { const { snapshot } = await api.getRun(id!, receipt.key);if (snapshot.state === 'terminal' && Object.values(snapshot.physical).every(v => v === 0)) { const value = snapshot.result!.ports.find(p => p.endpoint.node === 'dictionary')!.value as { status: string; packet: { root: { text: string } } };expect(value.status).toBe('data');if (value.status !== 'data') throw Error('missing data');return { receipt, request: JSON.parse(value.packet.root.text as string) }; }if (Date.now() > end) throw Error('dictionary HTTP did not settle');await new Promise(resolve => setTimeout(resolve, 30)); }
  }
  try {
    const file = readFileSync(packed), header = Buffer.alloc(12);header.writeUInt32LE(1);header.writeBigUInt64LE(BigInt(file.length), 4);
    const review = await post('/api/node-packages/review', Buffer.concat([header, file]), 'application/octet-stream');
    expect(review.executionPermissionGranted).toBe(false);await post('/api/node-packages/commit', JSON.stringify({ reviewId: review.reviewId, allowDowngrade: false, allowSameVersionRebuild: false }), 'application/json');
    id = (await api.open()).connectionId;const pending = await api.prepare(id, graph);expect(pending.requests.map(r => r.kind).sort()).toEqual(['execute', 'inputFlow', 'resourceRead']);
    const read = pending.requests.find(r => r.kind === 'resourceRead')!;expect(read.resource).toMatchObject({ binding: 'dictionary', reference: 'echo' });expect(JSON.stringify(pending)).not.toContain(resources);
    await api.approve(id, pending, pending.missing.filter(r => r.kind !== 'resourceRead').map(r => r.id), true);await expect(api.activate(id, pending.reviewId)).rejects.toBeInstanceOf(ExternalApiError);
    const approved = await api.prepare(id, graph);expect(approved.missing.map(r => r.kind)).toEqual(['resourceRead']);await api.approve(id, approved, approved.missing.map(r => r.id), true);
    writeFileSync(dictionary, first);await api.activate(id, approved.reviewId);const original = await result('1');
    expect(Object.keys(original.request.operation.resources)).toEqual(['echo']);expect(Buffer.from(original.request.operation.resources.echo).toString()).toBe(first);expect(original.request.operation.inputs.in.packet.root.text).toBe(input);
    writeFileSync(dictionary, 'changed-http-dictionary');expect(Buffer.from((await result('2')).request.operation.resources.echo).toString()).toBe(first);
    await api.revoke(id, read.id);await expect(api.getRun(id, original.receipt.key)).rejects.toBeInstanceOf(ExternalApiError);
    const refreshed = await api.prepare(id, graph);expect(refreshed.missing.map(r => r.kind)).toEqual(['resourceRead']);await api.approve(id, refreshed, refreshed.missing.map(r => r.id), false);await api.activate(id, refreshed.reviewId);
    expect(Buffer.from((await result('1')).request.operation.resources.echo).toString()).toBe('changed-http-dictionary');
    const stored = readFileSync(join(folder, 'permissions/permissions.json'), 'utf8');for (const secret of [resources, first, input, 'echo']) expect(stored).not.toContain(secret);
  } finally {
    if (id) await api.close(id);await middleware.close();server.closeAllConnections();await new Promise<void>(resolve => server.close(() => resolve()));
    if (dirname(folder) !== resolve(tmpdir()) || !folder.startsWith(join(tmpdir(), 'diyime-dictionary-http-real-'))) throw Error('unexpected fixture cleanup scope');rmSync(folder, { recursive: true });
  }
}, 180000);
