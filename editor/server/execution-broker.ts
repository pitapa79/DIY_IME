/** One explicit broker Host for v3 HTTP Sessions. Legacy v2 ownership is separate. */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { resolve } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { MAX_RESPONSE_TIMEOUT_MS } from './session.ts';

export const V3_POLICY_ENV = 'DIY_IME_V3_RESOURCE_POLICY';
const MAX_REQUEST = 1024 * 1024, MAX_RESPONSE = 8 * 1024 * 1024;
const MAX_ID = 18446744073709551615n;
export interface WireState {
  revision: string; activePlanVersion: string | null; preparedToken: string | null;
  canRollback: boolean; latestInputRevision: number | null; deliveryGeneration: string;
}
type ObjectValue = Record<string, unknown>;
interface ErrorValue { code: string; message: string }
interface BrokerReply { broker: 1; id: string; ok: boolean; data?: unknown; error?: ErrorValue }
interface CoreReply { protocol: 3; id: string; ok: boolean; data?: unknown; error?: ErrorValue; state: WireState }
export class ExecutionBridgeError extends Error {
  constructor(public code: string, message: string, public status = 503, public state?: WireState) { super(message); }
}
function object(value: unknown): value is ObjectValue { return !!value && typeof value === 'object' && !Array.isArray(value); }
function keys(value: ObjectValue, names: string[]) { return Object.keys(value).length === names.length && names.every(name => Object.hasOwn(value, name)); }
function decimal(value: unknown, zero = false): value is string {
  return typeof value === 'string' && value.length <= 20 && (zero && value === '0' || /^[1-9][0-9]*$/.test(value)) && BigInt(value) <= MAX_ID;
}
function epoch(value: unknown): value is string { return typeof value === 'string' && /^[0-9a-f]{32}$/.test(value); }
function errorValue(value: unknown): value is ErrorValue { return object(value) && keys(value, ['code','message']) && typeof value.code === 'string' && typeof value.message === 'string'; }
function outcome(value: ObjectValue) { return typeof value.ok === 'boolean' && (value.ok ? Object.hasOwn(value,'data') && !Object.hasOwn(value,'error') : !Object.hasOwn(value,'data') && errorValue(value.error)); }
function state(value: unknown): value is WireState {
  return object(value) && keys(value, ['revision','activePlanVersion','preparedToken','canRollback','latestInputRevision','deliveryGeneration']) &&
    decimal(value.revision, true) && (value.activePlanVersion === null || decimal(value.activePlanVersion)) &&
    (value.preparedToken === null || typeof value.preparedToken === 'string' && value.preparedToken.length > 0) &&
    typeof value.canRollback === 'boolean' && (value.latestInputRevision === null || Number.isSafeInteger(value.latestInputRevision) && Number(value.latestInputRevision) > 0) && decimal(value.deliveryGeneration);
}
const POLICY_FIELDS = ['maxActiveRuns','maxPhysicalJobs','maxObservers','maxQueuedJobs','maxQueuedInputBytes','maxRetainedRuns','maxRetainedBytes','maxResultBytesPerRun','retentionMs','cancelGraceMs','maxDedupeEntries','maxTombstones','queueWaitMs'];
function policy(value: unknown): value is ObjectValue {
  return object(value) && keys(value, POLICY_FIELDS) && POLICY_FIELDS.every(name => name === 'queueWaitMs' && value[name] === null || Number.isSafeInteger(value[name]) && Number(value[name]) >= 0);
}
interface Pending { resolve: (reply: BrokerReply) => void; reject: (error: Error) => void; control: boolean }
export interface BrokerConfiguration { core: string; root: string; policyFile: string; responseTimeoutMs: number | null }
export interface ExitInfo { code: number | null; signal: NodeJS.Signals | null }

/** Never kill/restart this child on a timeout or a Session disconnect. Its Rust
 * root owns the file lease and live producers until true ending. */
export class ExecutionBroker {
  readonly child: ChildProcessWithoutNullStreams;
  readonly exited: Promise<ExitInfo>;
  private pending = new Map<string, Pending>();
  private normalPending = 0;
  private sequence = 0n;
  private buffer = Buffer.alloc(0);
  private failure?: ExecutionBridgeError;
  private requestedShutdown = false;
  constructor(configuration: BrokerConfiguration, onExit: (cleanShutdown: boolean) => void = () => {}) {
    this.child = spawn(configuration.core, ['broker-v3', configuration.policyFile, configuration.policyFile + '.broker-lock'], { cwd: configuration.root, windowsHide: true, stdio: 'pipe' });
    this.child.on('error', error => this.fail(new ExecutionBridgeError('core-unavailable', `v3 Hostを起動できません。${error.message}`)));
    this.child.stdin.on('error', () => this.fail(new ExecutionBridgeError('host-lost', 'v3 Hostへの通信が失われました。結果は不明です。')));
    this.child.stderr.resume(); // Raw stderr may contain input or credentials; do not retain it.
    this.child.stdout.on('data', (chunk: Buffer) => this.consume(chunk));
    this.exited = new Promise(resolveExit => this.child.once('close', (code, signal) => {
      this.fail(new ExecutionBridgeError('host-lost', `v3 Hostが終了しました。`));
      resolveExit({ code, signal }); onExit(this.requestedShutdown && code === 0);
    }));
  }
  get healthy() { return !this.failure; }
  get closedError() { return this.failure ?? new ExecutionBridgeError('host-lost', 'v3 Hostを利用できません。'); }
  request(request: ObjectValue, control = false): Promise<BrokerReply> {
    if (this.failure) return Promise.reject(this.failure);
    // Four control slots remain available for closing the four live Sessions.
    if (this.pending.size >= 16 || !control && this.normalPending >= 12) return Promise.reject(new ExecutionBridgeError('busy', 'v3 Hostの要求枠が使用中です。'));
    const next = this.sequence + 1n;
    if (next > MAX_ID) return Promise.reject(new ExecutionBridgeError('id-exhausted', 'v3通信IDを使い切りました。'));
    const id = next.toString(), line = JSON.stringify({ broker: 1, id, request });
    if (Buffer.byteLength(line) > MAX_REQUEST) return Promise.reject(new ExecutionBridgeError('request-limit', 'broker包絡を含む要求が1 MiBを超えています。', 413));
    this.sequence = next;
    if (!control) this.normalPending++;
    return new Promise((resolveReply, reject) => {
      this.pending.set(id, { resolve: resolveReply, reject, control });
      this.child.stdin.write(line + '\n', error => { if (error) this.fail(new ExecutionBridgeError('host-lost', 'v3 Hostへの送信に失敗しました。結果は不明です。')); });
    });
  }
  private consume(chunk: Buffer) {
    if (this.failure) return; // Keep draining stdout without accumulating it.
    let offset = 0;
    while (offset < chunk.length) {
      const end = chunk.indexOf(10, offset), stop = end < 0 ? chunk.length : end;
      const part = chunk.subarray(offset, stop);
      if (this.buffer.length + part.length > MAX_RESPONSE + 1) { this.fail(new ExecutionBridgeError('host-lost', 'broker返信が8 MiBを超えました。')); return; }
      this.buffer = Buffer.concat([this.buffer, part]);
      if (end < 0) return;
      let frame = this.buffer; this.buffer = Buffer.alloc(0);
      if (frame.at(-1) === 13) frame = frame.subarray(0, -1);
      try {
        if (frame.length > MAX_RESPONSE) throw new Error('reply limit');
        const reply: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(frame));
        if (!object(reply) || !keys(reply, ['broker','id','ok', reply.ok ? 'data' : 'error']) || reply.broker !== 1 || !decimal(reply.id) || !outcome(reply)) throw new Error('reply shape');
        const pending = this.pending.get(reply.id); if (!pending) throw new Error('unexpected id');
        this.pending.delete(reply.id); if (!pending.control) this.normalPending--;
        pending.resolve(reply as unknown as BrokerReply);
      } catch { this.fail(new ExecutionBridgeError('host-lost', 'broker返信の版・ID・形式が一致しません。結果は不明です。')); return; }
      offset = end + 1;
    }
  }
  fail(error: ExecutionBridgeError) {
    if (this.failure) return;
    this.failure = error; this.buffer = Buffer.alloc(0);
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear(); this.normalPending = 0;
    // EOF is handled by Rust's root drain; kill would lose physical ownership.
    this.child.stdin.end();
  }
  shutdown() { this.requestedShutdown = true; this.fail(new ExecutionBridgeError('host-closed', 'v3 Hostへの新規受理を終了しました。')); }
}
interface HostOwner { broker: ExecutionBroker; references: number }
const hosts = new Map<string, HostOwner>();
function acquire(configuration: BrokerConfiguration): { key: string; owner: HostOwner } {
  const policyFile = realpathSync(resolve(configuration.root, configuration.policyFile));
  const key = JSON.stringify([resolve(configuration.core), realpathSync(configuration.root), policyFile]);
  let owner = hosts.get(key);
  if (owner) {
    if (!owner.broker.healthy) throw owner.broker.closedError;
    owner.references++; return { key, owner };
  }
  const broker = new ExecutionBroker({ ...configuration, policyFile }, clean => { if (clean && hosts.get(key)?.broker === broker) hosts.delete(key); });
  owner = { broker, references: 1 }; hosts.set(key, owner); return { key, owner };
}
function release(host: { key: string; owner: HostOwner }) { if (--host.owner.references === 0) host.owner.broker.shutdown(); }
interface Session { nextId: bigint }

export class ExecutionSessionPool {
  private host?: { key: string; owner: HostOwner };
  private sessions = new Map<string, Session>();
  private opening = 0;
  private closed = false;
  private domain?: string;
  private resourcePolicy?: ObjectValue;
  constructor(private configuration: BrokerConfiguration) {
    if (!configuration.policyFile.trim()) throw new ExecutionBridgeError('host-unconfigured','v3 policyファイルを明示してください。');
    const timeout = configuration.responseTimeoutMs;
    if (timeout !== null && (!Number.isSafeInteger(timeout) || timeout < 1 || timeout > MAX_RESPONSE_TIMEOUT_MS)) throw new Error('v3 response timeout is invalid');
  }
  get hostPid() { return this.host?.owner.broker.child.pid; }
  get hostExited() { return this.host?.owner.broker.exited; }
  private broker() {
    if (this.closed) throw new ExecutionBridgeError('host-closed', 'bridgeのv3所有者は終了しています。');
    if (!this.host) { try { this.host = acquire(this.configuration); } catch (error) { if (error instanceof ExecutionBridgeError) throw error; throw new ExecutionBridgeError('host-unconfigured', `明示したv3 policyを読めません。${error instanceof Error ? error.message : String(error)}`); } }
    if (!this.host.owner.broker.healthy) throw this.host.owner.broker.closedError;
    return this.host.owner.broker;
  }
  private data(reply: BrokerReply): unknown {
    if (!reply.ok) throw new ExecutionBridgeError(reply.error!.code, reply.error!.message, reply.error!.code === 'busy' ? 503 : 422);
    return reply.data;
  }
  private invalid(): never {
    const error = new ExecutionBridgeError('host-lost', 'v3のepoch・domain・policy・状態形式が一致しません。');
    this.host?.owner.broker.fail(error); throw error;
  }
  private async core(epochId: string, request: ObjectValue): Promise<CoreReply> {
    const session = this.sessions.get(epochId); if (!session) throw new ExecutionBridgeError('session-lost', 'v3 Sessionは終了しています。新規Sessionには旧結果を復元しません。');
    const next = session.nextId + 1n; if (next > MAX_ID) throw new ExecutionBridgeError('id-exhausted', 'v3 Sessionの通信IDを使い切りました。');
    const id = next.toString(); session.nextId = next;
    const data = this.data(await this.broker().request({ method: 'request', sessionEpoch: epochId, message: { protocol: 3, id, request } }));
    if (!object(data) || !keys(data,['protocol','id','ok',data.ok ? 'data' : 'error','state']) || data.protocol !== 3 || data.id !== id || !outcome(data) || !state(data.state)) this.invalid();
    return data as unknown as CoreReply;
  }
  private watch<T>(operation: Promise<T>, signal: AbortSignal | undefined, abandon: () => void): Promise<T> {
    return new Promise((resolveResult,reject) => {
      let settled = false, timer: ReturnType<typeof setTimeout> | undefined;
      const finish = (error?: unknown, result?: T) => { if (settled) return; settled = true; if (timer !== undefined) clearTimeout(timer); signal?.removeEventListener('abort',abort); if (error) reject(error); else resolveResult(result!); };
      const lost = (message: string) => { if (settled) return; abandon(); finish(new ExecutionBridgeError('session-lost', message)); };
      const abort = () => lost('送信後にHTTP接続が失われました。処理結果は不明です。');
      if (this.configuration.responseTimeoutMs !== null) timer = setTimeout(() => lost('v3応答の監視期限を超えました。処理結果は不明です。'),this.configuration.responseTimeoutMs);
      signal?.addEventListener('abort',abort,{ once:true });
      operation.then(result => finish(undefined,result),error => finish(error));
      if (signal?.aborted) abort();
    });
  }
  async create(signal?: AbortSignal) {
    if (signal?.aborted) throw new ExecutionBridgeError('aborted','送信前に取り消しました。',499);
    const broker = this.broker();
    if (this.sessions.size + this.opening >= 4) throw new ExecutionBridgeError('session-limit','v3 Sessionは同時4件までです。');
    this.opening++; let reserved = true, epochId: string | undefined, abandoned = false;
    const operation = broker.request({ method:'open-session' }).then(async reply => {
      const data = this.data(reply);
      if (!object(data) || !keys(data,['sessionEpoch','resourceDomain','resourcePolicy']) || !epoch(data.sessionEpoch) || typeof data.resourceDomain !== 'string' || !data.resourceDomain || !policy(data.resourcePolicy)) this.invalid();
      if (this.domain !== undefined && (this.domain !== data.resourceDomain || !isDeepStrictEqual(this.resourcePolicy,data.resourcePolicy))) this.invalid();
      this.domain = data.resourceDomain; this.resourcePolicy = data.resourcePolicy;
      epochId = data.sessionEpoch; this.opening--; reserved = false;
      this.sessions.set(epochId,{ nextId:0n });
      if (abandoned || this.closed) { void this.remove(epochId).catch(() => {}); throw new ExecutionBridgeError('session-lost','作成中に接続が失われました。'); }
      const hello = await this.core(epochId,{ method:'hello' });
      if (!hello.ok || !object(hello.data) || hello.data.sessionEpoch !== epochId || hello.data.resourceDomain !== this.domain || !isDeepStrictEqual(hello.data.resourcePolicy,this.resourcePolicy)) this.invalid();
      if (this.closed || !this.sessions.has(epochId)) throw new ExecutionBridgeError('session-lost','作成中にSessionが閉じられました。');
      return { id:epochId, resourceDomain:this.domain, resourcePolicy:this.resourcePolicy, state:hello.state };
    }).catch(error => { if (epochId) void this.remove(epochId).catch(() => {}); throw error; }).finally(() => { if (reserved) this.opening--; });
    return this.watch(operation,signal,() => { abandoned = true; if (epochId) void this.remove(epochId).catch(() => {}); });
  }
  async call(epochId: string, request: ObjectValue, signal?: AbortSignal) {
    if (signal?.aborted) throw new ExecutionBridgeError('aborted','送信前に取り消しました。',499);
    const reply = await this.watch(this.core(epochId,request),signal,() => { void this.remove(epochId).catch(() => {}); });
    if (!this.sessions.has(epochId)) throw new ExecutionBridgeError('session-lost','応答待ちの間にSessionが閉じられました。');
    if (!reply.ok) throw new ExecutionBridgeError(reply.error!.code,reply.error!.message,reply.error!.code === 'busy' ? 503 : 422,reply.state);
    return { state:reply.state, result:reply.data };
  }
  async remove(epochId: string) {
    if (!this.sessions.delete(epochId)) return;
    const broker = this.host?.owner.broker; if (!broker?.healthy) return;
    const reply = await this.watch(broker.request({ method:'close-session', sessionEpoch:epochId },true),undefined,() => {});
    const data = this.data(reply); if (!object(data) || !keys(data,['closed']) || data.closed !== true) this.invalid();
  }
  close() {
    if (this.closed) return;
    for (const id of [...this.sessions.keys()]) void this.remove(id).catch(() => {});
    this.closed = true; if (this.host) release(this.host);
  }
}
