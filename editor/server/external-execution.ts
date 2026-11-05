import { randomBytes } from 'node:crypto';
import { PackageManager, type PackageManagerOptions } from './node-packages.ts';
import { BridgeError } from './session.ts';
export const EXTERNAL_HOST_CONFIG_ENV = 'DIYIME2_EXTERNAL_HOST_CONFIG';
const fields: Record<string, string[]> = {
  traceStart: [], traceStartDetailed: [], traceRead: ['after', 'limit'], traceStop: [], traceClear: [],
  secrets: [], secretPut: ['id', 'value'], secretDelete: ['id'], definitions: [], status: [], saved: [], prepare: ['graph'], approve: ['reviewId', 'ids', 'save'], activate: ['reviewId'],
  run: ['request'], getRun: ['key'], cancelRun: ['key'], getStart: ['startKey'], invalidateInput: ['expectedPlanVersion', 'revision'],
  closeSession: [], protection: ['enabled'], revoke: ['id'], cancel: [],
};
export function externalOperation(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new BridgeError('external-request', '要求形式が異なります。', 422);
  const object = value as Record<string, unknown>, method = object.method;
  if (typeof method !== 'string' || !Object.hasOwn(fields, method) || Object.keys(object).some(k => k !== 'method' && !fields[method].includes(k))
    || !fields[method].every(k => Object.hasOwn(object, k))) throw new BridgeError('external-request', '許可されていない要求です。', 422);
  return object;
}
/** One UI owner, fixed executable/config. A late cleanup never addresses a new owner. */
export class ExternalExecutionPool {
  private current?: { id: string; manager: PackageManager; generation: object };
  private closing?: Promise<void>;
  private failed = false;
  private disposed = false;
  constructor(private readonly options: PackageManagerOptions) {}
  async create(signal?: AbortSignal) {
    if (this.disposed || this.failed) throw new BridgeError('external-ended', '実行Hostが終了しました。アプリを再起動してください。', 503);
    if (this.current || this.closing) throw new BridgeError('external-owned', '他の接続を終了するまでお待ちください。', 409);
    const owner = { id: randomBytes(16).toString('hex'), manager: new PackageManager(this.options, 'execution'), generation: {} };
    this.current = owner;
    try { const status = await owner.manager.call({ method: 'status' }, undefined, signal);return { connectionId: owner.id, status }; }
    catch (error) { await this.remove(owner.id);throw error; }
  }
  async call(id: string, value: unknown, signal?: AbortSignal) {
    const operation = externalOperation(value), owner = this.current;
    if (!owner || owner.id !== id) throw new BridgeError('external-owner', '現在の接続ではありません。', 409);
    if (operation.method === 'cancel') { owner.manager.cancelCurrent();return { cancelRequested: true }; }
    const control = operation.method === 'protection' || operation.method === 'revoke';
    if (control || operation.method === 'secretPut' || operation.method === 'secretDelete') owner.generation = {};
    const generation = owner.generation;
    const result = await owner.manager.call(operation, undefined, signal, control);
    if (this.current !== owner || generation !== owner.generation) throw new BridgeError('external-context-changed', '応答は取り下げられました。', 409);
    return result;
  }
  remove(id: string): Promise<void> {
    const owner = this.current;
    if (!owner || owner.id !== id) return Promise.resolve();
    this.current = undefined;owner.generation = {};
    const ending = owner.manager.close().then(() => { if (!owner.manager.cleanExit) this.failed = true; }).finally(() => { if (this.closing === ending) this.closing = undefined; });
    this.closing = ending;return ending;
  }
  async close() { this.disposed = true;if (this.current) await this.remove(this.current.id);await this.closing; }
}
