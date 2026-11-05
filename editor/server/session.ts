import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import type { SessionState } from '../src/model.ts';

export class BridgeError extends Error {
  constructor(public code: string, message: string, public status = 422, public state?: SessionState) { super(message); }
}
export const DEFAULT_RESPONSE_TIMEOUT_MS = 10_000;
export const MAX_RESPONSE_TIMEOUT_MS = 2_147_483_647;
export const RESPONSE_TIMEOUT_ENV = 'DIY_IME_BRIDGE_RESPONSE_TIMEOUT_MS';

function validateResponseTimeout(value: number | null): number | null {
  if (value !== null && (!Number.isSafeInteger(value) || value < 1 || value > MAX_RESPONSE_TIMEOUT_MS)) {
    throw new Error(`${RESPONSE_TIMEOUT_ENV}: 1〜${MAX_RESPONSE_TIMEOUT_MS} の整数ミリ秒、または unlimited を指定してください。`);
  }
  return value;
}

/** 未設定だけを既定値とする。空文字や数値の暗黙変換は許可しない。 */
export function readResponseTimeout(value: string | undefined): number | null {
  if (value === undefined) return DEFAULT_RESPONSE_TIMEOUT_MS;
  if (value === 'unlimited') return null;
  if (!/^[1-9][0-9]*$/.test(value) || value.trim() !== value) throw new Error(`${RESPONSE_TIMEOUT_ENV}: 正の整数ミリ秒、または unlimited を指定してください。`);
  return validateResponseTimeout(Number(value));
}

interface Reply { protocol: number; id: number | null; ok: boolean; data?: unknown; error?: { code: string; message: string }; state: SessionState }
interface Pending { id: number; finish: (error?: Error, reply?: Reply) => void }

/** 1プロセスへ直列に要求を送る。送信後に切断した場合は結果不明なのでセッション全体を無効にする。 */
export class PersistentCore {
  readonly child: ChildProcessWithoutNullStreams;
  private pending?: Pending;
  private buffer = Buffer.alloc(0);
  private queue: Promise<unknown> = Promise.resolve();
  private queued = 0;
  private sequence = 0;
  private closed = false;
  lastUsed = Date.now();

  constructor(core: string, root: string, private onClose: () => void = () => {}, private responseTimeoutMs: number | null = DEFAULT_RESPONSE_TIMEOUT_MS) {
    validateResponseTimeout(responseTimeoutMs);
    this.child = spawn(core, ['session'], { cwd: root, windowsHide: true, stdio: 'pipe' });
    this.child.on('error', error => this.close(new BridgeError('core-unavailable', `常駐コアを起動できません。cargo build --bin diy-ime2 で更新してください。 (${error.message})`, 503)));
    this.child.on('close', () => this.close(new BridgeError('session-lost', `常駐コアとの接続が終了しました。再接続後に計画を準備し直してください。`, 503)));
    this.child.stderr.resume(); // Drain without retaining or copying raw diagnostics into replies.
    this.child.stdin.on('error', error => this.close(new BridgeError('session-lost', `コアへの送信に失敗しました。${error.message}`, 503)));
    this.child.stdout.on('data', (chunk: Buffer) => {
      if (this.closed) return;
      if (this.buffer.length + chunk.length > 8 * 1024 * 1024) { this.close(new BridgeError('session-lost', '応答が 8 MiB を超えたため接続を終了しました。', 503)); return; }
      this.buffer = Buffer.concat([this.buffer, chunk]);
      let end: number;
      while ((end = this.buffer.indexOf(10)) !== -1) {
        const line = this.buffer.subarray(0, end); this.buffer = this.buffer.subarray(end + 1);
        try {
          const reply = JSON.parse(line.toString('utf8')) as Reply;
          const framedInputError = reply.id === null && reply.ok === false && reply.error?.code === 'protocol-json';
          if (!this.pending || reply.protocol !== 2 || (reply.id !== this.pending.id && !framedInputError) || typeof reply.ok !== 'boolean' || !Number.isSafeInteger(reply.state?.revision)) throw new Error('unexpected response');
          this.pending.finish(undefined, reply);
        } catch { this.close(new BridgeError('session-lost', '応答形式またはプロトコルの版が一致しません。Rust コアをビルドし直してください。', 503)); return; }
      }
    });
  }

  async request(request: Record<string, unknown>, signal?: AbortSignal): Promise<Reply> {
    if (this.queued >= 16) throw new BridgeError('busy', 'このセッションの要求が混み合っています。', 503);
    this.queued++; this.lastUsed = Date.now();
    const task = this.queue.catch(() => {}).then(() => {
      if (this.closed) throw new BridgeError('session-lost', '接続が失われています。再接続してください。', 503);
      if (signal?.aborted) throw new BridgeError('aborted', '送信前に要求を取り消しました。', 499);
      const id = ++this.sequence;
      const line = JSON.stringify({ protocol: 2, id, request });
      if (Buffer.byteLength(line) > 1024 * 1024) throw new BridgeError('request-limit', '要求が 1 MiB を超えています。', 413);
      return new Promise<Reply>((resolve, reject) => {
        const abort = () => this.close(new BridgeError('session-lost', '要求の途中で通信が切れました。処理結果を確認できないため、再接続が必要です。', 503));
        const timer = this.responseTimeoutMs === null ? undefined : setTimeout(() => this.close(new BridgeError('session-lost', `コアが ${this.responseTimeoutMs} ms 以内に応答しなかったため接続を終了しました。`, 503)), this.responseTimeoutMs);
        const pending: Pending = { id, finish: (error, reply) => {
          if (this.pending !== pending) return;
          this.pending = undefined; if (timer !== undefined) clearTimeout(timer); signal?.removeEventListener('abort', abort);
          if (error) reject(error); else resolve(reply!);
        } };
        this.pending = pending;
        signal?.addEventListener('abort', abort, { once: true });
        this.child.stdin.write(line + '\n');
      });
    });
    this.queue = task;
    try { return await task; } finally { this.queued--; this.lastUsed = Date.now(); }
  }

  get hasRequests() { return this.queued > 0; }

  close(error = new BridgeError('session-lost', 'セッションを終了しました。', 503)) {
    if (this.closed) return;
    this.closed = true; this.pending?.finish(error); this.child.kill(); this.onClose();
  }
}

export class SessionPool {
  private sessions = new Map<string, PersistentCore>();
  private timer: ReturnType<typeof setInterval>;
  constructor(private core: string, private root: string, private responseTimeoutMs: number | null = DEFAULT_RESPONSE_TIMEOUT_MS) {
    validateResponseTimeout(responseTimeoutMs);
    this.timer = setInterval(() => {
      for (const [id, session] of this.sessions) if (!session.hasRequests && Date.now() - session.lastUsed > 15 * 60_000) this.remove(id);
    }, 60_000).unref();
  }
  async create(signal?: AbortSignal) {
    if (this.sessions.size >= 4) throw new BridgeError('session-limit', '同時に開ける常駐セッションは4件までです。不要なタブを閉じてください。', 503);
    const id = randomUUID();
    const session = new PersistentCore(this.core, this.root, () => this.sessions.delete(id), this.responseTimeoutMs);
    this.sessions.set(id, session);
    try { return { id, state: (await session.request({ method: 'status' }, signal)).state }; }
    catch (error) { this.remove(id); throw error; }
  }
  async call(id: string, request: Record<string, unknown>, signal?: AbortSignal) {
    const session = this.sessions.get(id);
    if (!session) throw new BridgeError('session-lost', 'セッションが終了しています。再接続して計画を準備し直してください。', 503);
    const reply = await session.request(request, signal);
    if (!reply.ok) throw new BridgeError(reply.error?.code ?? 'core-error', reply.error?.message ?? 'コアの要求が失敗しました。', 422, reply.state);
    return { state: reply.state, result: reply.data };
  }
  remove(id: string) { this.sessions.get(id)?.close(); this.sessions.delete(id); }
  close() { clearInterval(this.timer); for (const session of [...this.sessions.values()]) session.close(); this.sessions.clear(); }
}
