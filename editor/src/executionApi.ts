import { executionDefinitions } from './executionPayload';
import type { Graph } from './model';
import { runtimeInputError } from './model';
import { decimal, epoch, errorValue, exact, object, runKey, runSnapshot, safeInteger, sameRun, wireState,
  type ExecutionReply, type ExecutionRunKey, type ExecutionSession, type ExecutionSnapshot,
  type ExecutionStart, type ExecutionReceipt, type ExecutionWireState } from './executionModel';

export class ExecutionApiError extends Error {
  constructor(public code: string, message: string, public resultUnknown: boolean, public state?: ExecutionWireState) { super(message); }
}
const invalid = (): never => { throw new ExecutionApiError('invalid-response', 'v3応答を確認できません。結果は不明です。', true); };
const invalidRequest = (): never => { throw new ExecutionApiError('invalid-request', 'v3要求の識別子・入力・予算を確認してください。', false); };
function checked<T>(value: unknown, guard: (value: unknown) => value is T): T { return guard(value) ? value : invalid(); }
const POLICY_FIELDS = ['maxActiveRuns', 'maxPhysicalJobs', 'maxObservers', 'maxQueuedJobs', 'maxQueuedInputBytes',
  'maxRetainedRuns', 'maxRetainedBytes', 'maxResultBytesPerRun', 'retentionMs', 'cancelGraceMs', 'maxDedupeEntries', 'maxTombstones', 'queueWaitMs'];
function resourcePolicy(value: unknown): value is Record<string, number | null> {
  return exact(value, POLICY_FIELDS) && POLICY_FIELDS.every(name =>
    name === 'queueWaitMs' && value[name] === null || safeInteger(value[name], !['maxQueuedJobs', 'maxTombstones'].includes(name)))
    && (value.maxResultBytesPerRun as number) <= (value.maxRetainedBytes as number);
}
function session(value: unknown): value is ExecutionSession {
  return exact(value, ['id', 'resourceDomain', 'resourcePolicy', 'state']) && epoch(value.id)
    && typeof value.resourceDomain === 'string' && value.resourceDomain.length > 0 && wireState(value.state)
    && resourcePolicy(value.resourcePolicy);
}
function receipt(value: unknown): value is ExecutionReceipt {
  return exact(value, ['state', 'replayed', 'key']) && value.state === 'accepted' && typeof value.replayed === 'boolean' && runKey(value.key);
}
function budget(value: unknown) { return value === null || safeInteger(value, true); }
export function createExecutionApi(fetcher: typeof fetch = fetch) {
  async function request<T>(path: string, method: string, body: unknown, decode: (value: unknown) => T, signal?: AbortSignal): Promise<T> {
    if (signal?.aborted) throw new ExecutionApiError('aborted', '送信前に取り消しました。', false);
    let encoded: string | undefined;
    try { encoded = body === undefined ? undefined : JSON.stringify(body); } catch { return invalidRequest(); }
    let response: Response, value: unknown;
    try {
      response = await fetcher(path, { method, signal, ...(encoded === undefined ? {} : { headers: { 'Content-Type': 'application/json' }, body: encoded }),
        ...(method === 'DELETE' ? { keepalive: true } : {}) });
      value = await response.json();
    } catch { throw new ExecutionApiError('connection-lost', 'v3接続の応答を確認できません。結果は不明です。', true); }
    if (!object(value) || typeof value.ok !== 'boolean') return invalid();
    if (!value.ok) {
      if (response.ok || !exact(value, ['ok', 'error'], ['state']) || !errorValue(value.error)) return invalid();
      const state = Object.hasOwn(value, 'state') ? checked(value.state, wireState) : undefined;
      if (state !== undefined && ![422, 503].includes(response.status)) return invalid();
      throw new ExecutionApiError(value.error.code, value.error.message, state === undefined, state);
    }
    if (!response.ok || !exact(value, ['ok', 'data'])) return invalid();
    return decode(value.data);
  }
  const path = (id: string) => { if (!epoch(id)) return invalidRequest(); return `/api/v3/sessions/${id}`; };
  async function call<T>(id: string, body: unknown, decode: (value: unknown) => T, signal?: AbortSignal): Promise<ExecutionReply<T>> {
    return request(path(id) + '/request', 'POST', body, value => {
      if (!exact(value, ['state', 'result'])) return invalid();
      return { state: checked(value.state, wireState), result: decode(value.result) };
    }, signal);
  }
  const empty = (value: unknown) => exact(value, []) ? {} : invalid();
  const keyResult = (value: unknown, id: string) => exact(value, ['key']) && runKey(value.key) && value.key.sessionEpoch === id ? { key: value.key } : invalid();
  const snapshotResult = (value: unknown, key: ExecutionRunKey) => runSnapshot(value) && sameRun(value.key, key) ? value : invalid();
  const keyBody = (method: string, key: ExecutionRunKey) => { if (!runKey(key)) return invalidRequest(); return { method, key }; };
  const plan = (version: string) => { if (!decimal(version)) return invalidRequest(); return version; };
  return {
    definitions: (id: string, signal?: AbortSignal) => call(id, { method: 'definitions' }, value => checked(value, executionDefinitions), signal),
    validate: (id: string, graph: Graph, signal?: AbortSignal) => call(id, { method: 'validate', graph }, value => exact(value, ['valid', 'order']) && value.valid === true && Array.isArray(value.order) && value.order.every(v => typeof v === 'string') ? { valid: true as const, order: value.order as string[] } : invalid(), signal),
    open: (signal?: AbortSignal) => request('/api/v3/sessions', 'POST', {}, value => checked(value, session), signal),
    close: (id: string) => request(path(id), 'DELETE', undefined, value => exact(value, ['closed']) && value.closed === true ? { closed: true as const } : invalid()),
    status: (id: string, signal?: AbortSignal) => request(path(id), 'GET', undefined, value => {
      if (!exact(value, ['state', 'result']) || !exact(value.result, ['activeGraph', 'preparedGraph', 'preparedOrder'])) return invalid();
      // Graph payloads remain unknown until the editor validates/loads them.
      return { state: checked(value.state, wireState), result: value.result };
    }, signal),
    prepare: (id: string, graph: Graph, signal?: AbortSignal) => call(id, { method: 'prepare', graph }, value =>
      exact(value, ['token']) && typeof value.token === 'string' && value.token.length > 0 ? { token: value.token } : invalid(), signal),
    activate: (id: string, token: string, signal?: AbortSignal) => call(id, { method: 'activate', token }, empty, signal),
    rollback: (id: string, version: string, signal?: AbortSignal) => call(id, { method: 'rollback', expectedPlanVersion: plan(version) }, empty, signal),
    invalidate: (id: string, version: string, revision: number, signal?: AbortSignal) => {
      if (!safeInteger(revision, true)) return Promise.reject(new ExecutionApiError('invalid-request', '入力の更新番号を確認してください。', false));
      return call(id, { method: 'invalidate-input', expectedPlanVersion: plan(version), revision }, empty, signal);
    },
    start: (id: string, start: ExecutionStart, signal?: AbortSignal) => {
      if (!object(start) || !decimal(start.startKey) || !decimal(start.expectedPlanVersion)
        || start.input !== undefined && (!object(start.input) || runtimeInputError(start.input) !== undefined)
        || start.runBudgetMs !== undefined && !budget(start.runBudgetMs)
        || start.jobBudgets !== undefined && (!Array.isArray(start.jobBudgets) || !start.jobBudgets.every(job => object(job) && typeof job.nodeId === 'string' && budget(job.budgetMs)))) return invalidRequest();
      return call(id, { ...start, method: 'start-run' }, value => receipt(value) && value.key.sessionEpoch === id
        && value.key.planVersion === start.expectedPlanVersion && value.key.inputRevision === start.input?.revision ? value : invalid(), signal);
    },
    getStart: (id: string, startKey: string, signal?: AbortSignal) => { plan(startKey); return call(id, { method: 'get-start', startKey }, value => keyResult(value, id), signal); },
    getRun: (id: string, key: ExecutionRunKey, signal?: AbortSignal) => call<ExecutionSnapshot>(id, keyBody('get-run', key), value => snapshotResult(value, key), signal),
    cancel: (id: string, key: ExecutionRunKey, signal?: AbortSignal) => call<ExecutionSnapshot>(id, keyBody('cancel-run', key), value => snapshotResult(value, key), signal),
  };
}
