/** ローカル試作用の CLI ブリッジ。Rust の検査・実行結果をそのまま返す。 */
import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { IncomingMessage, ServerResponse } from 'node:http';
import type { Connect, Plugin } from 'vite';
import { BridgeError, SessionPool, readResponseTimeout, RESPONSE_TIMEOUT_ENV } from './session.ts';
import { ExecutionBridgeError, ExecutionSessionPool, V3_POLICY_ENV } from './execution-broker.ts';
import { PackageManager, NODE_MANAGER_CONFIG_ENV, type PackageManagerOptions } from './node-packages.ts';

import { ExternalExecutionPool, EXTERNAL_HOST_CONFIG_ENV } from './external-execution.ts';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const CORE = resolve(ROOT, 'target/debug', process.platform === 'win32' ? 'diy-ime2.exe' : 'diy-ime2');
const MAX_BODY = 1024 * 1024;
const MAX_OUTPUT = 8 * 1024 * 1024;
let active = 0;

function send(response: ServerResponse, status: number, value: unknown) {
  if (response.destroyed || response.writableEnded) return;
  response.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' });
  response.end(JSON.stringify(value));
}
async function body(request: IncomingMessage): Promise<string> {
  if (!request.headers['content-type']?.startsWith('application/json')) throw new BridgeError('content-type', 'JSON 形式で送信してください。', 415);
  if (Number(request.headers['content-length'] ?? 0) > MAX_BODY) throw new BridgeError('body-limit', 'グラフは 1 MiB 以下にしてください。', 413);
  let size = 0; const chunks: Buffer[] = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > MAX_BODY) throw new BridgeError('body-limit', 'グラフは 1 MiB 以下にしてください。', 413);
    chunks.push(Buffer.from(chunk));
  }
  const text = Buffer.concat(chunks).toString('utf8');
  try { JSON.parse(text); } catch { throw new BridgeError('graph-json', 'JSON の構文が正しくありません。'); }
  return text;
}

export async function invokeCore(command: 'list-nodes' | 'validate-graph' | 'run', input: string, signal?: AbortSignal, responseTimeoutMs = readResponseTimeout(process.env[RESPONSE_TIMEOUT_ENV]), transport = { core: CORE, root: ROOT }): Promise<unknown> {
  if (active >= 4) throw new BridgeError('busy', '実行中です。少し待って再実行してください。', 503);
  active++;
  try {
    return await new Promise((accept, reject) => {
      const child = spawn(transport.core, command === 'list-nodes' ? [command] : [command, '-'], { cwd: transport.root, windowsHide: true, stdio: 'pipe' });
      const out: Buffer[] = [], err: Buffer[] = []; let bytes = 0, done = false;
      const finish = (error?: Error, data?: unknown) => {
        if (done) return; done = true; if (timer !== undefined) clearTimeout(timer); signal?.removeEventListener('abort', abort);
        if (error) { child.kill(); reject(error); } else accept(data);
      };
      const abort = () => finish(new BridgeError('aborted', '実行を取り消しました。', 499));
      const timer = responseTimeoutMs === null ? undefined : setTimeout(() => finish(new BridgeError('timeout', `実行が ${responseTimeoutMs} ms を超えました。`, 504)), responseTimeoutMs);
      signal?.addEventListener('abort', abort, { once: true });
      if (signal?.aborted) { abort(); return; }
      child.on('error', error => finish(new BridgeError('core-unavailable', `Rust コアを起動できません。先に cargo build --bin diy-ime2 を実行してください。 (${error.message})`, 503)));
      const collect = (chunks: Buffer[], chunk: Buffer) => { bytes += chunk.length; if (bytes > MAX_OUTPUT) finish(new BridgeError('output-limit', '実行結果が大きすぎます。', 413)); else chunks.push(chunk); };
      child.stdout.on('data', chunk => collect(out, chunk)); child.stderr.on('data', chunk => collect(err, chunk));
      child.stdin.on('error', () => {}); // 早期の起動失敗・キャンセル時の EPIPE は上の終了処理で扱う。
      child.on('close', code => {
        if (done) return;
        const stdout = Buffer.concat(out).toString('utf8');
        try {
          const data = JSON.parse(stdout);
          // 実行時エラーのレポートも構造化データとして UI へ返す。
          if (code === 0 || (command === 'run' && data && data.succeeded === false)) { finish(undefined, data); return; }
        } catch { /* コンパイル・設定・起動エラーは stderr から表示する。 */ }
        const message = Buffer.concat(err).toString('utf8').trim() || 'Rust コアから有効な結果を受信できませんでした。';
        finish(new BridgeError(message.match(/^([a-z-]+):/)?.[1] ?? 'core-error', message));
      });
      child.stdin.end(input);
    });
  } finally { active--; }
}

interface ApiOptions {
  /** Trusted server-side protocol fixture selection; never accepted from browser requests. */
  legacy?: { core: string; root: string };
  externalOnly?: boolean;
  examplesRoot?: string;
  external?: PackageManagerOptions;
  packages?: PackageManagerOptions;
  execution?: { policyFile: string; core?: string; root?: string; responseTimeoutMs?: number | null };
}
export function createApiMiddleware(options: ApiOptions = {}): Connect.NextHandleFunction & { close: () => Promise<void> } {
  const responseTimeoutMs = options.externalOnly ? null : readResponseTimeout(process.env[RESPONSE_TIMEOUT_ENV]);
  const sessions = new SessionPool(options.legacy?.core ?? CORE, options.legacy?.root ?? ROOT, responseTimeoutMs);
  const policyFile = options.externalOnly ? undefined : options.execution?.policyFile ?? process.env[V3_POLICY_ENV];
  const execution = policyFile === undefined ? undefined : new ExecutionSessionPool({
    core: options.execution?.core ?? CORE, root: options.execution?.root ?? ROOT,
    policyFile, responseTimeoutMs: options.execution?.responseTimeoutMs === undefined ? responseTimeoutMs : options.execution.responseTimeoutMs,
  });
  const packageConfig = options.packages?.config ?? process.env[NODE_MANAGER_CONFIG_ENV];
  const packages = packageConfig === undefined ? undefined : new PackageManager(options.packages ?? { config: packageConfig, executable: resolve(ROOT, 'target/debug', process.platform === 'win32' ? 'diy-ime2-node-manager.exe' : 'diy-ime2-node-manager') });
  const externalConfig = options.external?.config ?? process.env[EXTERNAL_HOST_CONFIG_ENV];
  const external = externalConfig === undefined ? undefined : new ExternalExecutionPool(options.external ?? { config: externalConfig, executable: resolve(ROOT, 'target/debug', process.platform === 'win32' ? 'diy-ime2-external-host.exe' : 'diy-ime2-external-host') });
  let packageBusy = false;
  const middleware: Connect.NextHandleFunction = (request, response, next) => {
    if (!request.url?.startsWith('/api/')) { next(); return; }
    void (async () => {
      // 開発サーバーは loopback のみ。同じオリジン以外からの API 利用も拒否する。
      const host = request.headers.host ?? '';
      if (!/^127\.0\.0\.1:\d+$/.test(host) || (request.headers.origin && request.headers.origin !== `http://${host}`)) throw new BridgeError('origin', 'このエディターからのリクエストのみ受け付けます。', 403);
      const pathname = new URL(request.url!, `http://${host}`).pathname;
      if (options.externalOnly && !pathname.startsWith('/api/external/') && pathname !== '/api/node-packages' && !pathname.startsWith('/api/node-packages/') && !pathname.startsWith('/api/examples/')) throw new BridgeError('external-required', 'この配布版は導入済みノードで実行します。', 403);
      const controller = new AbortController();
      response.on('close', () => { if (!response.writableEnded) controller.abort(); });
      let data: unknown;
      const sessionRoute = pathname.match(/^\/api\/sessions\/([0-9a-f-]{36})(?:\/(validate|prepare|activate|run|rollback))?$/);
      const v3Route = pathname.match(/^\/api\/v3\/sessions\/([0-9a-f]{32})(?:\/(request))?$/);
      const externalRoute = pathname.match(/^\/api\/external\/connections\/([0-9a-f]{32})(?:\/(request))?$/);
      if (pathname === '/api/external/connections' || externalRoute) {
        if (!external) throw new BridgeError('external-unconfigured', '導入済みノードの実行Hostが構成されていません。', 503);
        if (request.method === 'POST' && pathname === '/api/external/connections') {
          const input = JSON.parse(await body(request));
          if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length) throw new BridgeError('external-request', '空の要求を指定してください。', 422);
          data = await external.create(controller.signal);
        } else if (externalRoute) {
          const [, id, method] = externalRoute;
          if (request.method === 'DELETE' && !method) { await external.remove(id);data = { closed: true }; }
          else if (request.method === 'POST' && method === 'request') data = await external.call(id, JSON.parse(await body(request)), controller.signal);
          else throw new BridgeError('not-found', 'APIが見つかりません。', 404);
        } else throw new BridgeError('not-found', 'APIが見つかりません。', 404);
      }
      else if (pathname === '/api/node-packages' || pathname.startsWith('/api/node-packages/')) {
        if (!packages) {
          if (request.method === 'GET' && pathname === '/api/node-packages') data = { available: false };
          else throw new BridgeError('package-unavailable', 'ノード管理が構成されていません。', 503);
        } else if (request.method === 'POST' && pathname === '/api/node-packages/cancel') {
          const input: unknown = JSON.parse(await body(request));
          if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length) throw new BridgeError('package-request', '空の要求を指定してください。', 422);
          packages.cancelCurrent(); data = { cancelRequested: true };
        } else {
          if (packageBusy) throw new BridgeError('package-busy', '別のパッケージ処理が終了するまでお待ちください。', 409);
          packageBusy = true;
          try {
            if (request.method === 'GET' && pathname === '/api/node-packages') data = { ...await packages.call({ method: 'list' }, undefined, controller.signal) as object, available: true };
            else if (request.method === 'POST' && pathname === '/api/node-packages/review') {
              if (request.headers['content-type'] !== 'application/octet-stream') throw new BridgeError('content-type', 'パッケージのバイナリー形式で送信してください。', 415);
              if (Number(request.headers['content-length'] ?? 0) > packages.uploadBytes) throw new BridgeError('package-limit', '選択したファイルが導入サイズの設定を超えています。', 413);
              let size = 0; const chunks: Buffer[] = [];
              for await (const chunk of request) {
                size += chunk.length;
                if (size > packages.uploadBytes) throw new BridgeError('package-limit', '選択したファイルが導入サイズの設定を超えています。', 413);
                chunks.push(Buffer.from(chunk));
              }
              data = await packages.call({ method: 'review' }, Buffer.concat(chunks), controller.signal);
            } else if (request.method === 'POST' && ['/api/node-packages/commit', '/api/node-packages/discard'].includes(pathname)) {
              const input: unknown = JSON.parse(await body(request));
              if (!input || typeof input !== 'object' || Array.isArray(input)) throw new BridgeError('package-request', '確認結果を指定してください。', 422);
              const object = input as Record<string, unknown>, commit = pathname.endsWith('/commit');
              const allowed = commit ? ['reviewId', 'allowDowngrade', 'allowSameVersionRebuild'] : ['reviewId'];
              if (Object.keys(object).some(key => !allowed.includes(key)) || typeof object.reviewId !== 'string' || !/^[a-f0-9]{32}$/.test(object.reviewId)
                || (commit && (typeof object.allowDowngrade !== 'boolean' || typeof object.allowSameVersionRebuild !== 'boolean'))) throw new BridgeError('package-request', '確認結果の形式が正しくありません。', 422);
              data = await packages.call({ ...object, method: commit ? 'commit' : 'discard' }, undefined, controller.signal);
            } else throw new BridgeError('not-found', 'APIが見つかりません。', 404);
          } finally { packageBusy = false; }
        }
      }
      else if (pathname === '/api/v3/sessions' || v3Route) {
        if (!execution) throw new ExecutionBridgeError('host-unconfigured', `${V3_POLICY_ENV} に全13項目のpolicyファイルを明示してください。`);
        if (request.method === 'POST' && pathname === '/api/v3/sessions') {
          const input: unknown = JSON.parse(await body(request));
          if (!input || typeof input !== 'object' || Array.isArray(input) || Object.keys(input).length !== 0) throw new ExecutionBridgeError('protocol-request', 'Session作成には空のJSONオブジェクトを指定してください。', 422);
          data = await execution.create(controller.signal);
        } else if (v3Route) {
          const [, epoch, method] = v3Route;
          if (request.method === 'DELETE' && !method) { await execution.remove(epoch); data = { closed: true }; }
          else if (request.method === 'GET' && !method) data = await execution.call(epoch, { method: 'status' }, controller.signal);
          else if (request.method === 'POST' && method === 'request') {
            const input: unknown = JSON.parse(await body(request));
            if (!input || typeof input !== 'object' || Array.isArray(input)) throw new ExecutionBridgeError('protocol-request', 'JSONオブジェクトでv3要求を指定してください。', 422);
            data = await execution.call(epoch, input as Record<string, unknown>, controller.signal);
          } else throw new BridgeError('not-found', 'APIが見つかりません。', 404);
        } else throw new BridgeError('not-found', 'APIが見つかりません。', 404);
      }
      else if (request.method === 'POST' && pathname === '/api/sessions') data = await sessions.create(controller.signal);
      else if (sessionRoute) {
        const [, id, method] = sessionRoute;
        if (request.method === 'DELETE' && !method) { sessions.remove(id); data = { closed: true }; }
        else if (request.method === 'GET' && !method) data = await sessions.call(id, { method: 'status' }, controller.signal);
        else if (request.method === 'POST' && method) {
          const input = JSON.parse(await body(request));
          if (!input || typeof input !== 'object' || Array.isArray(input)) throw new BridgeError('protocol-request', 'JSON オブジェクトで要求してください。');
          data = await sessions.call(id, { ...input, method }, controller.signal);
        } else throw new BridgeError('not-found', 'API が見つかりません。', 404);
      }
      else if (request.method === 'GET' && pathname === '/api/nodes') data = await invokeCore('list-nodes', '', controller.signal, responseTimeoutMs, options.legacy);
      else if (request.method === 'GET' && /^\/api\/examples\/(official-dictionary|dictionary-exact|practical-pipeline|candidate-selection|stringify|runtime-romaji|runtime-input|weather|weather-negative|hello|branch)$/.test(pathname)) {
        data = JSON.parse(await readFile(resolve(options.examplesRoot ?? resolve(ROOT, 'examples/graphs'), `${pathname.split('/').at(-1)}.json`), 'utf8'));
      } else if (request.method === 'POST' && ['/api/graph/validate', '/api/graph/run'].includes(pathname)) {
        data = await invokeCore(pathname.endsWith('/run') ? 'run' : 'validate-graph', await body(request), controller.signal, responseTimeoutMs, options.legacy);
      } else throw new BridgeError('not-found', 'API が見つかりません。', 404);
      send(response, 200, { ok: true, data });
    })().catch(error => {
      const known = error instanceof BridgeError || error instanceof ExecutionBridgeError;
      send(response, known ? error.status : 500, { ok: false, error: { code: known ? error.code : 'bridge-error', message: error instanceof Error ? error.message : String(error) }, ...(known && error.state ? { state: error.state } : {}) });
    });
  };
  return Object.assign(middleware, { close: () => { sessions.close(); execution?.close(); return Promise.all([packages?.close(), external?.close()]).then(() => {}); } });
}

export function coreBridge(): Plugin {
  return { name: 'diy-ime2-local-core',
    configureServer(server) { const middleware = createApiMiddleware({ externalOnly: true }); server.middlewares.use(middleware); server.httpServer?.once('close', middleware.close); },
    configurePreviewServer(server) { const middleware = createApiMiddleware({ externalOnly: true }); server.middlewares.use(middleware); server.httpServer.once('close', middleware.close); },
  };
}
