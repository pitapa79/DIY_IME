import { decodeTracePage, traceDecimal } from './executionTrace';
import { decimal, epoch, exact, errorValue, runKey, runSnapshot, safeInteger, wireState, sameRun, type ExecutionRunKey, type ExecutionSnapshot, type ExecutionStart, type ExecutionWireState } from './executionModel';
import { executionDefinitions } from './executionPayload';
import type { Definition, Endpoint, Graph } from './model';
export interface ExternalStatus {
  phase: 'empty' | 'review' | 'ready' | 'protected'; protected: boolean; sessionEpoch: string | null;
  state: ExecutionWireState | null; reviewId: string | null; physical: Record<string, number>; workerProcesses: number;
  retainedWorker?: { physicalJobs: number; ready: boolean; processId: number | null; generation: number; activeInput: boolean; decodedReplies: number };
}
export interface Approval { id: string; kind: 'execute' | 'inputFlow' | 'resourceRead' | 'networkUse' | 'secretUse'; packageId: string; nodes: string[]; source: Endpoint | null; resource?: { binding: string; reference: string; policySha256: string }; network?: NetworkUse }
export interface NetworkUse {
  binding: string; method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'; destination: string; policySha256: string;
  limits: { maxCalls: number; maxRequestBytes: number; maxResponseBytes: number; maxContinuationBytes: number; budgetMs: number };
  secret?: SecretMetadata;
}
export interface SecretMetadata { id: string; version: string }
export interface ExternalReview { reviewId: string; sessionEpoch: string; requests: Approval[]; missing: Approval[] }
export class ExternalApiError extends Error { constructor(public code: string, message: string, public unknownResult = true) { super(message); } }
const invalid = (): never => { throw new ExternalApiError('external-response', '実行Hostの応答を確認できません。'); };
const hash = (v: unknown): v is string => typeof v === 'string' && /^[a-f0-9]{64}$/.test(v);
const endpoint = (v: unknown): v is Endpoint => exact(v, ['node', 'port']) && typeof v.node === 'string' && typeof v.port === 'string';
function status(v: unknown): ExternalStatus {
  if (!exact(v, ['phase', 'protected', 'sessionEpoch', 'state', 'reviewId', 'physical', 'workerProcesses'], ['retainedWorker']) || !['empty', 'review', 'ready', 'protected'].includes(String(v.phase))
    || typeof v.protected !== 'boolean' || !(v.sessionEpoch === null || epoch(v.sessionEpoch)) || !(v.state === null || wireState(v.state))
    || !(v.reviewId === null || epoch(v.reviewId)) || !v.physical || typeof v.physical !== 'object' || Array.isArray(v.physical) || !Object.values(v.physical).every(n => safeInteger(n)) || !safeInteger(v.workerProcesses)) return invalid();
  if (v.phase === 'ready' && (!epoch(v.sessionEpoch) || !wireState(v.state) || v.protected)) return invalid();
  if (Object.hasOwn(v, 'retainedWorker')) {
    const r = v.retainedWorker;
    if (!exact(r, ['physicalJobs', 'ready', 'processId', 'generation', 'activeInput', 'decodedReplies']) || !safeInteger(r.physicalJobs) || !safeInteger(r.decodedReplies)
      || !safeInteger(v.physical.physicalJobs) || r.physicalJobs > v.physical.physicalJobs || typeof r.ready !== 'boolean' || typeof r.activeInput !== 'boolean'
      || !safeInteger(r.generation) || !(r.processId === null || safeInteger(r.processId) && r.processId > 0 && r.processId <= 0xffffffff)) return invalid();
  }
  return v as unknown as ExternalStatus;
}
function approvals(v: unknown): Approval[] {
  if (!Array.isArray(v) || !v.every(r => {
    if (!exact(r, ['id', 'kind', 'packageId', 'nodes', 'source'], ['resource', 'network']) || !hash(r.id)
      || typeof r.packageId !== 'string' || !Array.isArray(r.nodes) || !r.nodes.every((n: unknown) => typeof n === 'string')) return false;
    if (r.kind === 'networkUse' || r.kind === 'secretUse') {
      if (r.source !== null || Object.hasOwn(r, 'resource') || !exact(r.network, ['binding', 'method', 'destination', 'policySha256', 'limits'], ['secret'])) return false;
      const n = r.network;
      if (typeof n.binding !== 'string' || !/^[a-z0-9_-][a-z0-9._-]{0,127}$/.test(n.binding) || n.binding.endsWith('.') || !['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].includes(String(n.method)) || typeof n.destination !== 'string' || !hash(n.policySha256)) return false;
      try { const url = new URL(n.destination);if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) return false; } catch { return false; }
      if (!exact(n.limits, ['maxCalls', 'maxRequestBytes', 'maxResponseBytes', 'maxContinuationBytes', 'budgetMs']) || !Object.values(n.limits).every(v => safeInteger(v) && v > 0)) return false;
      if (Object.hasOwn(n, 'secret')) secretMetadata(n.secret);else if (r.kind === 'secretUse') return false;
      return true;
    }
    if (Object.hasOwn(r, 'network')) return false;
    if (r.kind === 'resourceRead') return r.source === null && exact(r.resource, ['binding', 'reference', 'policySha256'])
      && typeof r.resource.binding === 'string' && typeof r.resource.reference === 'string' && hash(r.resource.policySha256);
    return !Object.hasOwn(r, 'resource') && (r.kind === 'execute' ? r.source === null : r.kind === 'inputFlow' && endpoint(r.source));
  }) || new Set(v.map(r => r.id)).size !== v.length) return invalid();
  return v;
}
function secretMetadata(v: unknown): SecretMetadata {
  if (!exact(v, ['id', 'version']) || typeof v.id !== 'string' || !/^[a-z0-9_-][a-z0-9._-]{0,127}$/.test(v.id) || v.id.endsWith('.') || !epoch(v.version)) return invalid();
  return v as unknown as SecretMetadata;
}
export function createExternalApi(fetcher: typeof fetch = fetch) {
  async function request<T>(path: string, method: string, body: unknown, decode: (v: unknown) => T): Promise<T> {
    let response: Response, value: unknown;
    try { response = await fetcher(path, { method, ...(method === 'DELETE' ? { keepalive: true } : {}), ...(body === undefined ? {} : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }) });value = await response.json(); }
    catch { throw new ExternalApiError('external-connection', '応答を確認できません。開始要求は再送せず照会してください。'); }
    if (exact(value, ['ok', 'error']) && value.ok === false && errorValue(value.error) && !response.ok) throw new ExternalApiError(value.error.code, value.error.message, response.status >= 500 || response.status === 499);
    if (!response.ok || !exact(value, ['ok', 'data']) || value.ok !== true) return invalid();
    return decode(value.data);
  }
  const path = (id: string) => epoch(id) ? '/api/external/connections/' + id : invalid();
  const call = <T,>(id: string, operation: Record<string, unknown>, decode: (v: unknown) => T) => request(path(id) + '/request', 'POST', operation, decode);
  const snapshot = (v: unknown, key: ExecutionRunKey) => {
    if (!exact(v, ['snapshot', 'state']) || !v.snapshot || typeof v.snapshot !== 'object') return invalid();
    const raw = v.snapshot as Record<string, unknown>;
    const suppressed = raw.current === false && raw.result === null;
    const normalized = suppressed ? { ...raw, ...(raw.state === 'terminal' ? { result: { reason: 'cancelled', delivery: 'cancelled', targets: [], ports: [], nodes: [], causes: [], recoveries: [] } } : {}) } : raw;
    if (suppressed && raw.state !== 'terminal') delete normalized.result;
    if (!runSnapshot(normalized) || !sameRun(normalized.key, key)) return invalid();
    const result: ExecutionSnapshot = { ...normalized };if (suppressed) delete result.result;
    return { snapshot: result, state: status(v.state) };
  };
  const trace=(value:unknown)=>{if(!exact(value,['trace']))return invalid();return decodeTracePage(value.trace);};
  return {
    traceStart:(id:string)=>call(id,{method:'traceStart'},trace),
    traceStartDetailed:(id:string)=>call(id,{method:'traceStartDetailed'},trace),
    traceRead:(id:string,after:string,limit=256)=>{if(!traceDecimal(after)||!Number.isInteger(limit)||limit<1||limit>256)return Promise.reject(new Error('記録の取得条件を確認してください。'));return call(id,{method:'traceRead',after,limit},trace);},
    traceStop:(id:string)=>call(id,{method:'traceStop'},trace),
    traceClear:(id:string)=>call(id,{method:'traceClear'},trace),
    open: () => request('/api/external/connections', 'POST', {}, v => exact(v, ['connectionId', 'status']) && epoch(v.connectionId) ? { connectionId: v.connectionId, status: status(v.status) } : invalid()),
    close: (id: string) => request(path(id), 'DELETE', undefined, v => exact(v, ['closed']) && v.closed === true ? undefined : invalid()),
    status: (id: string) => call(id, { method: 'status' }, status),
    definitions: (id: string) => call(id, { method: 'definitions' }, v => {
      if (!exact(v, ['definitions', 'state']) || !Array.isArray(v.definitions) || !executionDefinitions(v.definitions.map(definition => ({ version: 2, executionKind: 'deferred', definition, errorInputs: [] })))) return invalid();
      return { definitions: v.definitions as Definition[], state: status(v.state) };
    }),
    prepare: (id: string, graph: Graph) => call(id, { method: 'prepare', graph }, v => {
      if (!exact(v, ['reviewId', 'sessionEpoch', 'requests', 'missing']) || !epoch(v.reviewId) || !epoch(v.sessionEpoch)) return invalid();
      const requests = approvals(v.requests), missing = approvals(v.missing);
      if (!missing.every(r => requests.some(q => JSON.stringify(q) === JSON.stringify(r)))) return invalid();
      return { reviewId: v.reviewId, sessionEpoch: v.sessionEpoch, requests, missing };
    }),
    approve: (id: string, review: ExternalReview, ids: string[], save: boolean) => call(id, { method: 'approve', reviewId: review.reviewId, ids, save }, v => {
      if (!exact(v, ['reviewId', 'missing']) || v.reviewId !== review.reviewId) return invalid();
      const missing = approvals(v.missing);if (!missing.every(r => review.requests.some(q => JSON.stringify(q) === JSON.stringify(r)))) return invalid();return missing;
    }),
    activate: (id: string, reviewId: string) => call(id, { method: 'activate', reviewId }, status),
    start: (id: string, start: ExecutionStart) => call(id, { method: 'run', request: start }, v => {
      if (!exact(v, ['receipt', 'state']) || !exact(v.receipt, ['state', 'replayed', 'key']) || v.receipt.state !== 'accepted' || typeof v.receipt.replayed !== 'boolean' || !runKey(v.receipt.key)
        || v.receipt.key.planVersion !== start.expectedPlanVersion || v.receipt.key.inputRevision !== start.input?.revision) return invalid();
      const state = status(v.state);if (v.receipt.key.sessionEpoch !== state.sessionEpoch) return invalid();return { key: v.receipt.key, state };
    }),
    getRun: (id: string, key: ExecutionRunKey) => call(id, { method: 'getRun', key }, v => snapshot(v, key)),
    cancelRun: (id: string, key: ExecutionRunKey) => call(id, { method: 'cancelRun', key }, v => snapshot(v, key)),
    getStart: (id: string, startKey: string) => {
      if (!decimal(startKey)) return invalid();return call(id, { method: 'getStart', startKey }, v => {
        if (!exact(v, ['key', 'state']) || !runKey(v.key)) return invalid();const state = status(v.state);if (v.key.sessionEpoch !== state.sessionEpoch) return invalid();return { key: v.key, state };
      });
    },
    invalidate: (id: string, version: string, revision: number) => call(id, { method: 'invalidateInput', expectedPlanVersion: version, revision }, status),
    protect: (id: string, enabled: boolean) => call(id, { method: 'protection', enabled }, v => exact(v, ['protected']) && v.protected === enabled ? undefined : invalid()),
    revoke: (id: string, subject: string) => call(id, { method: 'revoke', id: subject }, v => exact(v, ['revoked']) && v.revoked === true ? undefined : invalid()),
    cancel: (id: string) => call(id, { method: 'cancel' }, v => exact(v, ['cancelRequested']) && v.cancelRequested === true ? undefined : invalid()),
    secrets: (id: string) => call(id, { method: 'secrets' }, v => {
      if (!exact(v, ['available', 'entries']) || typeof v.available !== 'boolean' || !Array.isArray(v.entries) || v.entries.length > 64) return invalid();
      const entries = v.entries.map(secretMetadata);if (new Set(entries.map(e => e.id)).size !== entries.length || !v.available && entries.length) return invalid();return { available: v.available, entries };
    }),
    secretPut: (id: string, name: string, value: string) => call(id, { method: 'secretPut', id: name, value }, v => {
      if (!exact(v, ['entry', 'state'])) return invalid();const entry = secretMetadata(v.entry);if (entry.id !== name) return invalid();return { entry, state: status(v.state) };
    }),
    secretDelete: (id: string, name: string) => call(id, { method: 'secretDelete', id: name }, v => exact(v, ['deleted', 'state']) && v.deleted === true ? status(v.state) : invalid()),
    saved: (id: string) => call(id, { method: 'saved' }, v => exact(v, ['ids']) && Array.isArray(v.ids) && v.ids.every(hash) ? v.ids as string[] : invalid()),
  };
}
export type ExternalClient = ReturnType<typeof createExternalApi>;
