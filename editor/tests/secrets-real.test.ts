import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
import { createExternalApi, ExternalApiError } from '../src/externalApi';
import type { Graph } from '../src/model';
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
it('manages only encrypted secrets through real HTTP, drains a live Worker before rotation and reopens without exporting a value', async () => {
  const folder = mkdtempSync(join(tmpdir(), 'diyime-secret-http-real-')), bin = join(ROOT, 'target/debug');
  const wasm = join(ROOT, 'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm');
  const worker = join(bin, 'diy-ime2-wasm-worker.exe'), dll = process.env.DIYIME2_TEST_WASMTIME_DLL ?? join(ROOT, 'target/tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll');
  const secretRoot = join(folder, 'secrets'), config = join(folder, 'host.json'), dataRoot = join(folder, 'data'), manifest = join(folder, 'manifest.json'), packed = join(folder, 'fixture.diynode');
  const definition = { kind: 'fixture.wasm', version: 1, inputs: [{ port: { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' }, required: true }], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } };
  writeFileSync(manifest, JSON.stringify({ packageId: 'test.secret.http', version: '1.0.0', displayName: 'Secret HTTP fixture', runtime: 'wasm32', guestApiVersion: 1, node: definition, moduleSha256: createHash('sha256').update(readFileSync(wasm)).digest('hex'), capabilities: [] }));
  for (const args of [['pack', manifest, wasm, packed], ['import', dataRoot, join(folder, 'install'), worker, dll, packed, '--commit']]) { const result = spawnSync(join(bin, 'diy-ime2-node-package.exe'), args, { encoding: 'utf8', windowsHide: true });expect(result.status, result.stderr).toBe(0); }
  const resourcePolicy = JSON.parse(readFileSync(join(ROOT, 'tests/contracts/execution_control.json'), 'utf8')).resourceProfiles.bounded;
  Object.assign(resourcePolicy, { maxPhysicalJobs: 2, maxQueuedJobs: 4, retentionMs: 60000 });
  writeFileSync(config, JSON.stringify({ packageManager: { dataRoot, workspace: join(folder, 'workers'), worker, runtimeDll: dll, policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: null, walltimeMs: null, terminationGraceMs: 5 } }, permissionRoot: join(folder, 'permissions'), secretRoot, resourcePolicy, requestBytes: 1048576, responseBytes: 16777216 }));
  const middleware = createApiMiddleware({ external: { executable: join(bin, 'diy-ime2-external-host.exe'), config } });
  const server = createServer((req, res) => middleware(req, res, () => { res.statusCode = 404;res.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));const address = server.address();if (!address || typeof address === 'string') throw Error('listen');
  const origin = 'http://127.0.0.1:' + address.port, api = createExternalApi((path, init) => fetch(origin + path, init));
  const first = 'private-api-secret-one-秘密', second = 'private-api-secret-two', text = 'private-runtime-input';
  const graph: Graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }, { id: 'worker', kind: 'fixture.wasm', version: 1, config: { mode: 'hold' } }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'worker', port: 'in' } }] };
  let id: string | undefined;
  try {
    id = (await api.open()).connectionId;expect(await api.secrets(id)).toEqual({ available: true, entries: [] });
    const initial = await api.secretPut(id, 'service', first);expect(initial.entry.id).toBe('service');expect(initial.state.phase).toBe('empty');
    const stored = readFileSync(join(secretRoot, 'secrets.json'), 'utf8');expect(stored).not.toContain(first);expect(stored).not.toContain('private-api-secret');
    for (const operation of [{ method: 'secretRead', id: 'service' }, { method: 'secrets', root: secretRoot }, { method: 'secretPut', id: 'service', value: 'invalid', root: secretRoot }, { method: 'secretDelete', id: 'service', version: initial.entry.version }]) {
      const response = await fetch(origin + '/api/external/connections/' + id + '/request', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(operation) });expect(response.status).toBe(422);
    }
    const review = await api.prepare(id, graph);await api.approve(id, review, review.missing.map(r => r.id), true);await api.activate(id, review.reviewId);
    const receipt = await api.start(id, { startKey: '1', expectedPlanVersion: '1', targets: [{ node: 'worker', port: 'out' }], input: { revision: 1, text, mode: 'compose' } });
    const end = Date.now() + 60000;while ((await api.status(id)).workerProcesses === 0) { if (Date.now() > end) throw Error('secret fixture Worker did not start');await new Promise(resolve => setTimeout(resolve, 30)); }
    const rotated = await api.secretPut(id, 'service', second);expect(rotated.entry.version).not.toBe(initial.entry.version);expect(rotated.state.workerProcesses).toBe(0);expect(rotated.state.physical.physicalJobs).toBe(0);expect(rotated.state.phase).toBe('empty');
    await expect(api.getRun(id, receipt.key)).rejects.toBeInstanceOf(ExternalApiError);const listed = await api.secrets(id);expect(listed.entries).toEqual([rotated.entry]);for (const secret of [first, second, text, secretRoot]) expect(JSON.stringify(listed)).not.toContain(secret);
    await api.close(id);id = (await api.open()).connectionId;expect((await api.secrets(id)).entries).toEqual([rotated.entry]);
    await api.protect(id, true);const protectedPut = await api.secretPut(id, 'protected-service', 'synthetic-protected-key');expect(protectedPut.state.phase).toBe('protected');
    await api.secretDelete(id, 'service');await api.secretDelete(id, 'protected-service');expect((await api.secrets(id)).entries).toEqual([]);
    await api.close(id);id = (await api.open()).connectionId;expect((await api.secrets(id)).entries).toEqual([]);
    for (const secret of [first, second, text]) expect(readFileSync(join(folder, 'permissions/permissions.json'), 'utf8')).not.toContain(secret);
  } finally {
    if (id) await api.close(id);await middleware.close();server.closeAllConnections();await new Promise<void>(resolve => server.close(() => resolve()));
    if (dirname(folder) !== resolve(tmpdir()) || !folder.startsWith(join(tmpdir(), 'diyime-secret-http-real-'))) throw Error('unexpected fixture cleanup scope');rmSync(folder, { recursive: true });
  }
}, 180000);
