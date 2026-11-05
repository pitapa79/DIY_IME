import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, existsSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { createApiMiddleware } from '../server/bridge';
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
it('imports two packages through real HTTP, Host IPC and AppContainer without granting execution', async () => {
  const folder = mkdtempSync(join(tmpdir(), 'diyime-package-http-real-'));
  const bin = join(ROOT, 'target/debug');
  const module = join(ROOT, 'target/wasm32-unknown-unknown/release/diyime2_fixture_guest.wasm');
  const worker = join(bin, 'diy-ime2-wasm-worker.exe');
  const dll = process.env.DIYIME2_TEST_WASMTIME_DLL ?? join(ROOT, 'target/tools/wasmtime50/wasmtime-v50.0.1-x86_64-windows-c-api/lib/wasmtime.dll');
  const config = join(folder, 'config.json'), dataRoot = join(folder, 'data');
  writeFileSync(config, JSON.stringify({ dataRoot, workspace: join(folder, 'workers'), worker, runtimeDll: dll, policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: 50000000, walltimeMs: 30000, terminationGraceMs: 5000 } }));
  const files: Buffer[] = [];
  for (const kind of ['fixture.wasm', 'fixture.wasm.alt']) {
    const definition = { kind, version: 1, inputs: [{ port: { id: 'in', dataType: { id: 'text-packet', version: 1 }, language: 'ja-Latn', root: 'text' }, required: true }], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: { type: 'object' }, defaultConfig: { mode: 'echo' } };
    const manifest = join(folder, kind + '.json'), packed = join(folder, kind + '.diynode');
    writeFileSync(manifest, JSON.stringify({ packageId: kind, version: '1.0.0', displayName: kind, runtime: 'wasm32', guestApiVersion: 1, node: definition, moduleSha256: createHash('sha256').update(readFileSync(module)).digest('hex'), capabilities: [] }));
    const result = spawnSync(join(bin, 'diy-ime2-node-package.exe'), ['pack', manifest, module, packed], { windowsHide: true, encoding: 'utf8' });
    expect(result.status, result.stderr).toBe(0); files.push(readFileSync(packed));
  }
  const count = Buffer.alloc(4); count.writeUInt32LE(files.length);
  const chunks: Buffer[] = [count];
  for (const file of files) { const length = Buffer.alloc(8); length.writeBigUInt64LE(BigInt(file.length)); chunks.push(length, file); }
  const middleware = createApiMiddleware({ packages: { executable: join(bin, 'diy-ime2-node-manager.exe'), config } });
  const server = createServer((request, response) => middleware(request, response, () => { response.writeHead(404); response.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('missing listener');
  const origin = 'http://127.0.0.1:' + address.port;
  try {
    const review = await fetch(origin + '/api/node-packages/review', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: Buffer.concat(chunks) });
    const result = await review.json(); expect(review.status, JSON.stringify(result)).toBe(200);
    expect(result.data.review.changes).toHaveLength(2); expect(result.data.executionPermissionGranted).toBe(false);
    expect(existsSync(join(dataRoot, 'objects'))).toBe(false);
    const commit = await fetch(origin + '/api/node-packages/commit', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ reviewId: result.data.reviewId, allowDowngrade: false, allowSameVersionRebuild: false }) });
    const installed = await commit.json(); expect(commit.status, JSON.stringify(installed)).toBe(200);
    expect(installed.data.catalog.entries).toHaveLength(2); expect(installed.data.executionPermissionGranted).toBe(false);
    const list = await (await fetch(origin + '/api/node-packages')).json();
    expect(list.data.available).toBe(true); expect(list.data.catalog.entries).toHaveLength(2);
  } finally {
    await middleware.close();
    server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve()));
    if (dirname(folder) !== resolve(tmpdir()) || !folder.startsWith(join(tmpdir(), 'diyime-package-http-real-'))) throw new Error('unexpected test cleanup scope');
    rmSync(folder, { recursive: true });
  }
}, 60_000);