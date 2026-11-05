import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { mkdir, readFile, writeFile, lstat, realpath, unlink, rmdir } from 'node:fs/promises';
import { join, resolve, sep, isAbsolute } from 'node:path';
import { randomUUID, randomBytes } from 'node:crypto';
import { createApiMiddleware } from './bridge.ts';
import { PackageManager, type PackageManagerOptions } from './node-packages.ts';

export interface SetupTransport { call(operation: Record<string, unknown>): Promise<unknown>; cancelCurrent(): void; close(): Promise<void> }
export interface ApplicationOptions {
  applicationRoot: string; dataRoot: string; port?: number; leaseMs?: number;
  setupTransport?: (options: PackageManagerOptions) => SetupTransport;
}
interface SetupReport { installed: string[]; present: string[]; pending: string[]; conflicts: { packageId: string; nodeKind: string }[]; complete: boolean; executionPermissionGranted: false }
export interface ApplicationState { phase: 'preparing' | 'ready' | 'review' | 'failed' | 'closing'; prepared: number; total: 48; conflicts: string[] }
const POLICY = { maxActiveRuns: 4, maxPhysicalJobs: 2, maxObservers: 16, maxQueuedJobs: 8, maxQueuedInputBytes: 1048576, maxRetainedRuns: 8, maxRetainedBytes: 8388608, maxResultBytesPerRun: 1048576, retentionMs: 60000, cancelGraceMs: 5, maxDedupeEntries: 64, maxTombstones: 64, queueWaitMs: null };
const MIME: Record<string, string> = { '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.ico': 'image/x-icon' };
function report(value: unknown): SetupReport {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error('invalid setup report');
  const r = value as SetupReport;
  const ids = (x: unknown): x is string[] => Array.isArray(x) && x.length <= 48 && x.every(s => typeof s === 'string' && /^[a-z0-9._-]{1,256}$/.test(s));
  if (!ids(r.installed) || r.installed.length > 16 || !ids(r.present) || !ids(r.pending) || !Array.isArray(r.conflicts) || r.conflicts.length > 48 || !r.conflicts.every(c => c && typeof c === 'object' && ids([c.packageId, c.nodeKind])) || typeof r.complete !== 'boolean' || r.executionPermissionGranted !== false || r.installed.length + r.present.length + r.pending.length + r.conflicts.length !== 48 || r.complete !== (!r.pending.length && !r.conflicts.length)) throw Error('invalid setup report');
  return r;
}
async function privateDirectory(path: string) {
  await mkdir(path, { recursive: true, mode: 0o700 });
  const stat = await lstat(path); if (!stat.isDirectory() || stat.isSymbolicLink()) throw Error('storage unavailable');
}
async function boundedJson(path: string): Promise<unknown> {
  const stat = await lstat(path);if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1048576) throw Error('configuration unavailable');
  const bytes = await readFile(path);if (bytes.length > 1048576) throw Error('configuration unavailable');return JSON.parse(bytes.toString('utf8'));
}
function json(response: ServerResponse, status: number, data: unknown) {
  if (response.destroyed || response.writableEnded) return;
  response.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });response.end(JSON.stringify(data));
}
async function emptyJson(request: IncomingMessage) {
  if (request.headers['content-type'] !== 'application/json' || Number(request.headers['content-length'] ?? 0) > 64) throw Error('request');
  let size = 0;const chunks: Buffer[] = [];
  for await (const chunk of request) { size += chunk.length;if (size > 64) throw Error('request');chunks.push(Buffer.from(chunk)); }
  const value: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length) throw Error('request');
}
function setupPage(nonce: string) {
  return `<!doctype html><html lang="ja"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>DIY_IME2 の準備</title><body><main><h1>DIY_IME2</h1><p id="state">同梱ノードを準備しています。</p><p>初回の準備にはしばらくかかることがあります。入力や実行の許可は、この後に確認します。</p><button id="continue" hidden>現在のノードで開く</button><button id="close">終了</button></main><script nonce="${nonce}">
const text=document.getElementById('state'),next=document.getElementById('continue');let ended=false;
const post=path=>fetch(path,{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});
document.getElementById('close').onclick=async()=>{ended=true;text.textContent='処理の終了を待っています…';try{const response=await post('/api/app/close');if(!response.ok)throw Error();text.textContent='終了しました。画面を閉じてください。';}catch{text.textContent='終了を確認できませんでした。接続した処理の停止を待ちます。';}};
next.onclick=()=>location.href='/?continue=1';
async function tick(){if(ended)return;try{await post('/api/app/heartbeat');const s=await(await fetch('/api/app')).json();if(s.phase==='ready'){location.reload();return;}if(s.phase==='review'){text.textContent='異なる版のノードを保持しました。更新は「ノードの導入」で確認できます。';next.hidden=false;}else if(s.phase==='failed'){text.textContent='準備を完了できませんでした。アプリを終了して再起動してください。';}else text.textContent='同梱ノードの準備 '+s.prepared+' / '+s.total;}catch{text.textContent='接続が終了しました。アプリを起動し直してください。';return;}setTimeout(tick,1000);}void tick();
</script></body></html>`;
}
/** Trusted launcher supplies all paths. HTTP only controls the current application owner. */
export async function startApplication(options: ApplicationOptions) {
  if (!isAbsolute(options.applicationRoot) || !isAbsolute(options.dataRoot)) throw Error('absolute application paths required');
  const applicationRoot = await realpath(options.applicationRoot), dataRoot = resolve(options.dataRoot);
  await privateDirectory(dataRoot);
  for (const name of ['nodes', 'workers', 'permissions', 'secrets', 'dictionaries', 'runs']) await privateDirectory(join(dataRoot, name));
  const runDirectory = join(dataRoot, 'runs', randomUUID());await privateDirectory(runDirectory);
  const managerPath = join(runDirectory, 'manager.json'), executionPath = join(runDirectory, 'execution.json');
  const dictionary = join(dataRoot, 'dictionaries', 'dictionary.tsv');
  try { await writeFile(dictionary, await readFile(join(applicationRoot, 'defaults', 'dictionary.tsv')), { flag: 'wx', mode: 0o600 }); }
  catch (e) { if ((e as NodeJS.ErrnoException).code !== 'EEXIST') throw e; }
  let network: unknown;
  try { const settings = await boundedJson(join(dataRoot, 'settings.json'));if (!settings || typeof settings !== 'object' || Array.isArray(settings) || Object.keys(settings).some(k => k !== 'network')) throw Error('invalid settings');network = (settings as { network?: unknown }).network; }
  catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e; }
  const bin = join(applicationRoot, 'bin');
  const managerConfig = { officialRoot: join(applicationRoot, 'official'), dataRoot: join(dataRoot, 'nodes'), workspace: join(dataRoot, 'workers'), worker: join(bin, 'diy-ime2-wasm-worker.exe'), runtimeDll: join(applicationRoot, 'runtime', 'wasmtime.dll'), policy: { processMemoryBytes: 536870912, wasmMemoryBytes: 134217728, fuelPerCall: 50000000, walltimeMs: 30000, terminationGraceMs: 5 }, dictionaryBindings: { dictionary: { root: join(dataRoot, 'dictionaries'), referencePointer: '/path', maxFileBytes: 1048576, maxTotalBytes: 1048576, maxFiles: 4, budgetMs: 1000 } } };
  const hostConfig = { packageManager: managerConfig, permissionRoot: join(dataRoot, 'permissions'), secretRoot: join(dataRoot, 'secrets'), ...(network === undefined ? {} : { network }), resourcePolicy: POLICY, requestBytes: 1048576, responseBytes: 16777216 };
  await writeFile(managerPath, JSON.stringify(managerConfig), { flag: 'wx', mode: 0o600 });
  await writeFile(executionPath, JSON.stringify(hostConfig), { flag: 'wx', mode: 0o600 });
  const managerOptions = { executable: join(bin, 'diy-ime2-node-manager.exe'), config: managerPath };
  const setup = options.setupTransport?.(managerOptions) ?? new PackageManager(managerOptions);
  const api = createApiMiddleware({ externalOnly: true, examplesRoot: join(applicationRoot, 'examples', 'graphs'), packages: managerOptions, external: { executable: join(bin, 'diy-ime2-external-host.exe'), config: executionPath } });
  const nonce = randomBytes(24).toString('base64'), staticRoot = await realpath(join(applicationRoot, 'web'));
  let state: ApplicationState = { phase: 'preparing', prepared: 0, total: 48, conflicts: [] }, stopping: Promise<void> | undefined, draining: Promise<void> | undefined, lastBeat = Date.now();
  const leaseMs = options.leaseMs ?? 60000;if (!Number.isSafeInteger(leaseMs) || leaseMs < 100) throw Error('invalid application lease');
  let origin = '';
  const server = createServer((request, response) => {
    response.setHeader('Cache-Control', 'no-store');response.setHeader('X-Content-Type-Options', 'nosniff');response.setHeader('Cross-Origin-Resource-Policy', 'same-origin');
    response.setHeader('Content-Security-Policy', `default-src 'self'; script-src 'self' 'nonce-${nonce}'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`);
    void (async () => {
      if (request.headers.host !== origin.slice(7) || request.headers.origin && request.headers.origin !== origin) { json(response, 403, { error: 'origin' });return; }
      const raw = decodeURIComponent((request.url ?? '/').split('?')[0]);
      if (raw.includes('\\') || raw.split('/').includes('..') || raw.includes('\0')) { json(response, 404, { error: 'path' });return; }
      const url = new URL(request.url ?? '/', origin), path = url.pathname;
      if (request.method === 'GET' && path === '/api/app') { json(response, 200, state);return; }
      if (request.method === 'POST' && (path === '/api/app/heartbeat' || path === '/api/app/close')) {
        await emptyJson(request);
        if (path.endsWith('/heartbeat')) { lastBeat = Date.now();json(response, 200, { alive: !draining }); }
        else { await drain();response.once('finish', () => setImmediate(() => { void close(); }));json(response, 200, { closed: true }); }
        return;
      }
      if (state.phase === 'closing') { json(response, 503, { error: 'closed' });return; }
      if (path.startsWith('/api/')) {
        if (state.phase !== 'ready' && state.phase !== 'review') { json(response, 503, { error: 'preparing' });return; }
        api(request, response, () => json(response, 404, { error: 'not-found' }));return;
      }
      if (request.method !== 'GET' && request.method !== 'HEAD') { json(response, 405, { error: 'method' });return; }
      if (path === '/' && state.phase !== 'ready' && !(state.phase === 'review' && url.search === '?continue=1')) {
        response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });response.end(request.method === 'HEAD' ? undefined : setupPage(nonce));return;
      }
      const name = path === '/' ? 'index.html' : /^\/assets\/[a-zA-Z0-9_.-]+\.(js|css|svg|png|woff2|ico)$/.test(path) ? path.slice(1) : undefined;
      if (!name) { json(response, 404, { error: 'not-found' });return; }
      const file = await realpath(join(staticRoot, name));if (!file.startsWith(staticRoot + sep)) { json(response, 404, { error: 'path' });return; }
      const stat = await lstat(file);if (!stat.isFile() || stat.size > 8 * 1024 * 1024) throw Error('asset');
      const bytes = await readFile(file);if (bytes.length > 8 * 1024 * 1024) throw Error('asset');
      const extension = name.slice(name.lastIndexOf('.'));
      response.writeHead(200, { 'Content-Type': name === 'index.html' ? 'text/html; charset=utf-8' : MIME[extension] });response.end(request.method === 'HEAD' ? undefined : bytes);
    })().catch(() => json(response, 400, { error: 'request' }));
  });
  let closedResolve: () => void;const closed = new Promise<void>(resolve => { closedResolve = resolve; });
  const setupWork = async () => {
    try { for (let step = 0; step < 4 && !draining; step++) {
      const current = report(await setup.call({ method: 'bootstrap' }));
      if (draining) break;
      state = { phase: current.pending.length ? 'preparing' : current.complete ? 'ready' : 'review', prepared: current.present.length + current.installed.length, total: 48, conflicts: current.conflicts.map(c => c.nodeKind) };
      if (!current.pending.length) return;
    } if (!draining) state = { ...state, phase: 'failed' }; }
    catch { if (!draining) state = { ...state, phase: 'failed' }; }
    finally { await setup.close(); }
  };
  let setupTask: Promise<void> | undefined;
  const timer = setInterval(() => { if (Date.now() - lastBeat > leaseMs) void close(); }, Math.min(1000, leaseMs));timer.unref();
  function drain(): Promise<void> {
    if (!draining) draining = (async () => {
      state = { ...state, phase: 'closing' };clearInterval(timer);setup.cancelCurrent();
      await Promise.all([api.close(), setup.close()]);await setupTask;
    })();
    return draining;
  }
  function close(): Promise<void> {
    if (!stopping) stopping = (async () => {
      await drain();
      const httpClosed = new Promise<void>(resolve => server.close(() => resolve()));server.closeAllConnections();await httpClosed;
      await Promise.allSettled([unlink(managerPath), unlink(executionPath)]);await rmdir(runDirectory).catch(() => {});closedResolve();
    })();
    return stopping;
  }
  try { await new Promise<void>((accept, reject) => { server.once('error', reject);server.listen(options.port ?? 5175, '127.0.0.1', accept); }); }
  catch (e) { clearInterval(timer);await api.close();await setup.close();await unlink(managerPath);await unlink(executionPath);await rmdir(runDirectory);throw e; }
  const address = server.address();if (!address || typeof address === 'string') throw Error('listener');origin = `http://127.0.0.1:${address.port}`;
  setupTask = setupWork();
  return { url: origin + '/', close, closed, state: () => structuredClone(state) };
}