import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
import { createExternalApi } from '../src/externalApi';
import { decodeExecution } from '../src/executionPayload';
import type { Graph } from '../src/model';
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
it('uses explicit network and secret consent through real HTTP and fences a held response before rotating its credential', async () => {
  const folder = mkdtempSync(join(tmpdir(), 'diyime-network-http-real-')), bin = join(ROOT, 'target/debug');
  const wasm = join(ROOT, 'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm'), worker = join(bin, 'diy-ime2-wasm-worker.exe');
  const dll = process.env.DIYIME2_TEST_WASMTIME_DLL ?? join(ROOT, 'target/tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll');
  const records: { authorization: string | undefined; body: string; url: string | undefined }[] = [];let hold = false, release: (() => void) | undefined;
  const remote = createServer(async (req, res) => { const chunks: Buffer[] = [];for await (const chunk of req) chunks.push(Buffer.from(chunk));records.push({ authorization: req.headers.authorization, body: Buffer.concat(chunks).toString(), url: req.url });const bytes = Buffer.from('http-remote-reply');res.writeHead(200, { 'Content-Length': bytes.length });res.flushHeaders();if (hold) release = () => { res.end(bytes); };else res.end(bytes); });
  await new Promise<void>(resolve => remote.listen(0, '127.0.0.1', resolve));const address = remote.address();if (!address || typeof address === 'string') throw Error('listen');
  const definition = { kind: 'fixture.wasm', version: 1, inputs: [{ port: { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' }, required: true }], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } };
  const manifest = join(folder, 'manifest.json'), packed = join(folder, 'fixture.diynode'), dataRoot = join(folder, 'data'), config = join(folder, 'host.json');
  writeFileSync(manifest, JSON.stringify({ packageId: 'test.network.http', version: '1.0.0', displayName: 'Network HTTP fixture', runtime: 'wasm32', guestApiVersion: 1, node: definition, moduleSha256: createHash('sha256').update(readFileSync(wasm)).digest('hex'), capabilities: [{ id: 'network.request', binding: 'api' }, { id: 'secret.use', binding: 'api' }] }));
  function command(args: string[]) { const r = spawnSync(join(bin, 'diy-ime2-node-package.exe'), args, { encoding: 'utf8', windowsHide: true });expect(r.status, r.stderr).toBe(0); }
  command(['pack', manifest, wasm, packed]);command(['import', dataRoot, join(folder, 'install'), worker, dll, packed, '--commit']);
  const resourcePolicy = JSON.parse(readFileSync(join(ROOT, 'tests/contracts/execution_control.json'), 'utf8')).resourceProfiles.bounded;Object.assign(resourcePolicy, { maxPhysicalJobs: 1, maxQueuedJobs: 0, retentionMs: 60000 });
  writeFileSync(config, JSON.stringify({ packageManager: { dataRoot, workspace: join(folder, 'workers'), worker, runtimeDll: dll, policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: null, walltimeMs: null, terminationGraceMs: 5 } }, permissionRoot: join(folder, 'permissions'), secretRoot: join(folder, 'secrets'), resourcePolicy, requestBytes: 1048576, responseBytes: 16777216, network: { bindings: { api: { host: '127.0.0.1', port: address.port, secure: false, allowInsecureLoopback: true, method: 'POST', path: '/fixed', contentType: 'application/octet-stream', requestBytes: 4096, responseBytes: 4096, headerBytes: 4096, timeouts: { resolveMs: 3000, connectMs: 3000, sendMs: 3000, receiveMs: 15000, budgetMs: 20000 }, authentication: { secret: 'service', header: 'Authorization', prefix: 'Bearer ' } } }, limits: { maxCalls: 2, maxRequestBytes: 8192, maxResponseBytes: 8192, maxContinuationBytes: 1024, budgetMs: 60000 } } }));
  const middleware = createApiMiddleware({ external: { executable: join(bin, 'diy-ime2-external-host.exe'), config } });
  const server = createServer((req, res) => middleware(req, res, () => { res.statusCode = 404;res.end(); }));await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const local = server.address();if (!local || typeof local === 'string') throw Error('listen');
  const origin = 'http://127.0.0.1:' + local.port, api = createExternalApi((path, init) => fetch(origin + path, init));let id: string | undefined;
  const graph: Graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }, { id: 'worker', kind: 'fixture.wasm', version: 1, config: { mode: 'network' } }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'worker', port: 'in' } }] };
  const secret = 'synthetic-http-network-private-key';
  async function until<T>(check: () => Promise<T | null>): Promise<T> { const deadline = Date.now() + 60000;for (;;) { const result = await check();if (result) return result;if (Date.now() > deadline) throw Error('network HTTP did not settle');await new Promise(resolve => setTimeout(resolve, 20)); } }
  try {
    id = (await api.open()).connectionId;await api.secretPut(id, 'service', secret);
    const first = await api.prepare(id, graph);expect(first.requests.map(r => r.kind).sort()).toEqual(['execute', 'inputFlow', 'networkUse', 'secretUse']);expect(records).toHaveLength(0);
    await api.approve(id, first, first.missing.filter(r => r.kind === 'execute' || r.kind === 'inputFlow').map(r => r.id), true);await expect(api.activate(id, first.reviewId)).rejects.toMatchObject({ code: 'permission-required' });expect(records).toHaveLength(0);
    const review = await api.prepare(id, graph);expect(review.missing.map(r => r.kind).sort()).toEqual(['networkUse', 'secretUse']);await api.approve(id, review, review.missing.map(r => r.id), true);await api.activate(id, review.reviewId);
    const request = { startKey: '1', expectedPlanVersion: '1', targets: [{ node: 'worker', port: 'out' }], input: { revision: 1, text: 'http-private-input', mode: 'compose' as const } };
    const receipt = await api.start(id, request);const snapshot = await until(async () => { const r = (await api.getRun(id!, receipt.key)).snapshot;return r.state === 'terminal' && Object.values(r.physical).every(n => n === 0) ? r : null; });
    const output = decodeExecution(snapshot.result!, receipt.key)!.ports.find(p => p.endpoint.node === 'worker')!.value;
    if (output.status !== 'data' || output.packet.root.type !== 'text') throw Error('missing network result');
    expect(Buffer.from(JSON.parse(output.packet.root.text).operation.response.body).toString()).toBe('http-remote-reply');expect(JSON.stringify(snapshot)).not.toContain(secret);expect(records).toEqual([{ authorization: 'Bearer ' + secret, body: 'http-private-input', url: '/fixed' }]);
    const saved = await api.prepare(id, graph);expect(saved.missing).toHaveLength(0);await api.activate(id, saved.reviewId);hold = true;const held = await api.start(id, request);await until(async () => release ? true : null);
    const active = await api.status(id);expect(active.workerProcesses).toBe(0);expect(active.physical.physicalJobs).toBe(1);
    let done = false;const rotation = api.secretPut(id, 'service', secret + '-new').then(value => { done = true;return value; });await new Promise(resolve => setTimeout(resolve, 120));expect(done).toBe(false);
    release!();release = undefined;await rotation;await expect(api.getRun(id, held.key)).rejects.toBeTruthy();
    const changed = await api.prepare(id, graph);expect(changed.missing.some(r => r.kind === 'secretUse')).toBe(true);expect(JSON.stringify(changed)).not.toContain(secret);expect(records).toHaveLength(2);
  } finally { release?.();if (id) await api.close(id);await new Promise<void>(resolve => server.close(() => resolve()));await new Promise<void>(resolve => remote.close(() => resolve()));if (dirname(folder) !== resolve(tmpdir()) || !folder.startsWith(join(tmpdir(), 'diyime-network-http-real-'))) throw Error('unexpected fixture root');rmSync(folder, { recursive: true }); }
}, 240000);