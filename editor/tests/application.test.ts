import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { join, resolve, sep } from 'node:path';
import { tmpdir } from 'node:os';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { startApplication, type SetupTransport } from '../server/application';
const roots: string[] = [], apps: Awaited<ReturnType<typeof startApplication>>[] = [];
const ids = Array.from({ length: 48 }, (_, i) => 'official.test.' + i);
const complete = { installed: [], present: ids, pending: [], conflicts: [], complete: true, executionPermissionGranted: false };
async function fixture(setup?: SetupTransport, leaseMs?: number) {
  const root = await mkdtemp(join(tmpdir(), 'diyime-application-test-'));roots.push(root);
  const applicationRoot = join(root, 'app'), dataRoot = join(root, 'user');
  for (const dir of ['web/assets', 'defaults', 'examples/graphs']) await mkdir(join(applicationRoot, dir), { recursive: true });
  await writeFile(join(applicationRoot, 'web/index.html'), '<!doctype html><p>BUILT APPLICATION</p>');
  await writeFile(join(applicationRoot, 'web/assets/test.js'), 'export const test = true;');
  await writeFile(join(applicationRoot, 'defaults/dictionary.tsv'), 'reading\tsurface\tweight\nこうすい\t降水\t90\n');
  await writeFile(join(applicationRoot, 'examples/graphs/official-dictionary.json'), '{"version":1,"nodes":[],"edges":[]}');
  const transport = setup ?? { call: vi.fn(async () => complete), cancelCurrent: vi.fn(), close: vi.fn(async () => {}) };
  const app = await startApplication({ applicationRoot, dataRoot, port: 0, leaseMs, setupTransport: () => transport });apps.push(app);
  return { app, root, applicationRoot, dataRoot, transport };
}
async function ready(app: Awaited<ReturnType<typeof startApplication>>) { await vi.waitFor(() => expect(app.state().phase).not.toBe('preparing')); }
afterEach(async () => { for (const app of apps.splice(0)) await app.close();for (const root of roots.splice(0)) { const p = resolve(root);if (!p.startsWith(resolve(tmpdir()) + sep) || !p.split(/[\\/]/).at(-1)?.startsWith('diyime-application-test-')) throw Error('unsafe test cleanup');await rm(p, { recursive: true, force: true }); } });
describe('offline application boundary', () => {
  it('serves built assets and fixed samples, rejects legacy execution and foreign origins', async () => {
    const { app } = await fixture();await ready(app);
    expect(await (await fetch(app.url)).text()).toContain('BUILT APPLICATION');
    const script = await fetch(app.url + 'assets/test.js');expect(script.status).toBe(200);expect(script.headers.get('content-type')).toContain('javascript');
    expect(script.headers.get('content-security-policy')).toContain("frame-ancestors 'none'");
    expect((await fetch(app.url + 'api/examples/official-dictionary')).status).toBe(200);
    expect((await fetch(app.url + 'api/nodes')).status).toBe(403);
    expect((await fetch(app.url + 'api/v3/sessions', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status).toBe(403);
    expect((await fetch(app.url + 'api/app', { headers: { Origin: 'https://unrelated.example' } })).status).toBe(403);
    expect((await fetch(app.url + 'api/app/close', { method: 'POST', headers: { 'Content-Type': 'text/plain' }, body: '{}' })).status).toBe(400);
    expect((await fetch(app.url + 'api/app/close', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{"root":"elsewhere"}' })).status).toBe(400);
    for (const path of ['settings.json', 'runtime/server.mjs', 'assets/%2e%2e/settings.json', 'assets/test.js%00', 'assets/sub/file.js', 'assets/test.js/..%5csettings.json']) expect((await fetch(app.url + path)).status).not.toBe(200);
  });
  it('publishes progress and preserves conflicts without retrying them forever', async () => {
    let count = 0;
    const transport = { call: vi.fn(async () => ++count === 1 ? { ...complete, installed: ids.slice(0, 16), present: [], pending: ids.slice(16,47), conflicts: [{ packageId: ids[47], nodeKind: 'custom.node' }], complete: false } : count === 2 ? { ...complete, installed: ids.slice(16,32), present: ids.slice(0,16), pending: ids.slice(32,47), conflicts: [{ packageId: ids[47], nodeKind: 'custom.node' }], complete: false } : { ...complete, installed: ids.slice(32,47), present: ids.slice(0,32), conflicts: [{ packageId: ids[47], nodeKind: 'custom.node' }], complete: false }), cancelCurrent: vi.fn(), close: vi.fn(async () => {}) };
    const { app } = await fixture(transport);await ready(app);expect(count).toBe(3);expect(app.state()).toEqual({ phase: 'review', prepared: 47, total: 48, conflicts: ['custom.node'] });
    expect(await (await fetch(app.url)).text()).toContain('同梱ノードを準備');
    expect(await (await fetch(app.url + '?continue=1')).text()).toContain('BUILT APPLICATION');
  });
  it('refuses a setup response that claims permission or cannot account for the inventory', async () => {
    const { app } = await fixture({ call: async () => ({ ...complete, executionPermissionGranted: true }), cancelCurrent() {}, async close() {} });await ready(app);expect(app.state().phase).toBe('failed');expect((await fetch(app.url + 'api/external/connections', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status).toBe(503);
  });
  it('acknowledges close only after the owned setup physically ends', async () => {
    let release!: () => void, abort!: (error: Error) => void;
    const ended = new Promise<void>(resolve => { release = resolve; });const work = new Promise<unknown>((_, reject) => { abort = reject; });
    const transport = { call: () => work, cancelCurrent: () => abort(Error('cancelled')), close: () => ended };
    const { app } = await fixture(transport);let replied = false;
    const closing = fetch(app.url + 'api/app/close', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' }).then(async response => { replied = true;return response.json(); });
    await vi.waitFor(() => expect(app.state().phase).toBe('closing'));expect(replied).toBe(false);release();expect(await closing).toEqual({ closed: true });await app.closed;
  });
  it('retains the user dictionary across application restarts', async () => {
    const { app, dataRoot } = await fixture();await ready(app);
    expect(await readFile(join(dataRoot, 'dictionaries/dictionary.tsv'), 'utf8')).toContain('降水');
    await app.close();await writeFile(join(dataRoot, 'dictionaries/dictionary.tsv'), 'user-owned dictionary');
    const restarted = await startApplication({ applicationRoot: join(dataRoot, '..', 'app'), dataRoot, port: 0, setupTransport: () => ({ call: async () => complete, cancelCurrent() {}, async close() {} }) });apps.push(restarted);await ready(restarted);expect(await readFile(join(dataRoot, 'dictionaries/dictionary.tsv'), 'utf8')).toBe('user-owned dictionary');
  });
  it('expires the application owner when no live view renews its lease', async () => {
    const { app, transport } = await fixture(undefined, 120);await app.closed;expect(app.state().phase).toBe('closing');expect(transport.cancelCurrent).toHaveBeenCalled();
  });});