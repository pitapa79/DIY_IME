import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { BridgeError } from './session.ts';

export const NODE_MANAGER_CONFIG_ENV = 'DIYIME2_NODE_MANAGER_CONFIG';
export interface PackageManagerOptions { executable: string; config: string }
interface Pending {
  accept: (value: unknown) => void; reject: (error: Error) => void;
  signal?: AbortSignal; abort: () => void; aborted: boolean;
}
/** One trusted Host manager; browser data never chooses command, path or root. */
export class PackageManager {
  readonly uploadBytes: number;
  private readonly responseBytes: number;
  private readonly requestBytes: number;
  cleanExit = false;
  private child?: ChildProcessWithoutNullStreams;
  private pending = new Map<string, Pending>();
  private serial = 0n;
  private buffered = 0;
  private failure?: Error;
  private closing = false;
  private ended?: Promise<void>;
  private end?: () => void;
  constructor(private readonly options: PackageManagerOptions, role: 'package' | 'execution' = 'package') {
    if (statSync(options.config).size > 1024 * 1024) throw new Error('Node manager configuration exceeds limit');
    const config = JSON.parse(readFileSync(options.config, 'utf8')) as { limits?: { batchBytes?: unknown; catalogBytes?: unknown }; requestBytes?: unknown; responseBytes?: unknown };
    const batch = config.limits?.batchBytes ?? 512 * 1024 * 1024;
    if (typeof batch !== 'number' || !Number.isSafeInteger(batch) || batch < 0 || !Number.isSafeInteger(batch * 2 + 4)) throw new Error('Invalid Host package batch limit');
    this.uploadBytes = batch * 2 + 4;
    const catalog = config.limits?.catalogBytes ?? 16 * 1024 * 1024;
    if (typeof catalog !== 'number' || !Number.isSafeInteger(catalog) || catalog < 0 || !Number.isSafeInteger(catalog * 2 + 65536)) throw new Error('Invalid Host package catalog limit');
    this.responseBytes = role === 'execution' ? Number(config.responseBytes) : catalog * 2 + 65536;
    this.requestBytes = role === 'execution' ? Number(config.requestBytes) : 1024 * 1024;
    if (!Number.isSafeInteger(this.responseBytes) || this.responseBytes < 1024 || !Number.isSafeInteger(this.requestBytes) || this.requestBytes <= 0 || this.requestBytes > 0xffffffff) throw new Error('Invalid Host transport limits');
  }
  private start(): ChildProcessWithoutNullStreams {
    if (this.child) return this.child;
    if (this.closing || this.failure) throw this.failure ?? new BridgeError('package-closed', 'ノード管理を終了しています。', 503);
    const child = spawn(this.options.executable, [this.options.config], { windowsHide: true, stdio: 'pipe' });
    this.child = child;
    this.ended = new Promise(resolve => { this.end = resolve; });
    child.stdin.on('error', () => { this.fail(new BridgeError('package-transport', 'ノード管理との接続が終了しました。', 503)); });
    child.on('error', () => { this.fail(new BridgeError('package-unavailable', 'ノード管理を起動できません。', 503)); });
    child.stderr.on('data', () => {}); // Host diagnostics never become an unchecked HTTP response.
    child.stdout.on('data', (chunk: Buffer) => {
      if (this.failure) return;
      this.buffered += chunk.length;
      if (this.buffered > this.responseBytes) { this.fail(new BridgeError('package-output-limit', 'ノード管理の応答が大きすぎます。', 413)); return; }
      // StringDecoder is unnecessary only if decoding happens at complete byte lines.
      this.consume(chunk);
    });
    child.on('close', code => {
      this.cleanExit = this.closing && !this.failure && code === 0;
      this.closing = true;
      for (const pending of this.pending.values()) {
        pending.signal?.removeEventListener('abort', pending.abort);
        pending.reject(this.failure ?? new BridgeError('package-ended', 'ノード管理が終了しました。アプリを再起動してください。', 503));
      }
      this.pending.clear(); this.end?.();
    });
    return child;
  }
  private bytes = Buffer.alloc(0);
  private consume(chunk: Buffer) {
    this.bytes = Buffer.concat([this.bytes, chunk]);
    for (;;) {
      const end = this.bytes.indexOf(10); if (end < 0) break;
      const line = this.bytes.subarray(0, end);
      this.bytes = this.bytes.subarray(end + 1); this.buffered = this.bytes.length;
      let value: { id?: unknown; ok?: unknown; data?: unknown; error?: { code?: unknown; message?: unknown } };
      try { value = JSON.parse(line.toString('utf8')); } catch { this.fail(new BridgeError('package-protocol', 'ノード管理の応答を確認できません。', 503)); return; }
      if (typeof value.id !== 'string' || typeof value.ok !== 'boolean') { this.fail(new BridgeError('package-protocol', 'ノード管理の応答形式が異なります。', 503)); return; }
      const pending = this.pending.get(value.id);
      if (!pending) {
        if (value.id.startsWith('cancel_') || value.id.startsWith('close_')) continue;
        this.fail(new BridgeError('package-protocol', 'ノード管理の応答IDが異なります。', 503)); return;
      }
      this.pending.delete(value.id); pending.signal?.removeEventListener('abort', pending.abort);
      if (pending.aborted) pending.reject(new BridgeError('aborted', '処理を取り消しました。導入済み一覧を再確認してください。', 499));
      else if (value.ok) pending.accept(value.data);
      else pending.reject(new BridgeError(typeof value.error?.code === 'string' ? value.error.code : 'package-error', typeof value.error?.message === 'string' ? value.error.message : 'パッケージを確認できません。', 422));
    }
  }
  private frame(id: string, operation: Record<string, unknown>, payload: Buffer = Buffer.alloc(0)) {
    const json = Buffer.from(JSON.stringify({ id, operation }), 'utf8');
    if (json.length > this.requestBytes) throw new BridgeError('host-request-limit', '要求がHostの上限を超えています。', 413);
    const header = Buffer.alloc(12); header.writeUInt32LE(json.length); header.writeBigUInt64LE(BigInt(payload.length), 4);
    this.child!.stdin.write(header); this.child!.stdin.write(json);
    if (payload.length) this.child!.stdin.write(payload);
  }
  private fail(error: Error) {
    if (this.failure) return; this.failure = error;
    // No terminate/timeout fallback: true Host/Worker exit owns release.
    void this.close();
  }
  call(operation: Record<string, unknown>, payload?: Buffer, signal?: AbortSignal, control = false): Promise<unknown> {
    if (this.pending.size >= 16 || this.pending.size && !control) return Promise.reject(new BridgeError('package-busy', '別のパッケージ処理が終了するまでお待ちください。', 409));
    if (this.closing || this.failure) return Promise.reject(this.failure ?? new BridgeError('package-closed', 'ノード管理は終了しています。', 503));
    if (signal?.aborted) return Promise.reject(new BridgeError('aborted', '処理を取り消しました。', 499));
    this.start();
    const id = (++this.serial).toString();
    return new Promise((accept, reject) => {
      const pending: Pending = { accept, reject, signal, aborted: false, abort: () => {
        if (pending.aborted) return; pending.aborted = true;
        if (!this.closing) { try { this.frame('cancel_' + id, { method: 'cancel', requestId: id }); } catch { void this.close(); } }
      } };
      this.pending.set(id, pending); signal?.addEventListener('abort', pending.abort, { once: true });
      try { this.frame(id, operation, payload); } catch (error) {
        this.pending.delete(id);signal?.removeEventListener('abort', pending.abort);reject(error as Error);return;
      }
      if (signal?.aborted) pending.abort();
    });
  }
  cancelCurrent(): void { for (const pending of this.pending.values()) pending.abort(); }
  close(): Promise<void> {
    if (!this.closing) {
      this.closing = true;
      if (this.child) { try { this.frame('close_' + (++this.serial), { method: 'close' }); } catch { /* EOF still cancels/drains the Host when a close frame cannot fit. */ } finally { this.child.stdin.end(); } }
    }
    return this.ended ?? Promise.resolve();
  }
}