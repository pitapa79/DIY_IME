import type { Endpoint, RuntimeInput } from './model';

export interface ExecutionWireState {
  revision: string;
  activePlanVersion: string | null;
  preparedToken: string | null;
  canRollback: boolean;
  latestInputRevision: number | null;
  deliveryGeneration: string;
}
export interface ExecutionRunKey {
  sessionEpoch: string;
  planVersion: string;
  runId: string;
  inputRevision?: number;
  deliveryGeneration: string;
}
export type ExecutionPhase = 'accepted' | 'running' | 'terminal';
export interface ExecutionPhysical {
  running: number; cancelRequested: number; stalled: number; queued: number;
}
/** Payloads deliberately stay unknown until the v3 renderer decodes them.
 * They are not the legacy Report/PortResult types. */
export interface ExecutionResult {
  reason: 'completed' | 'cancelled' | 'expired' | 'failed';
  delivery: 'complete' | 'recovered' | 'no-output' | 'failed' | 'cancelled' | 'expired';
  targets: Endpoint[];
  ports: { endpoint: Endpoint; value: unknown }[];
  nodes: unknown[]; causes: unknown[]; recoveries: unknown[];
  error?: { code: string; message: string };
}
export interface ExecutionSnapshot {
  key: ExecutionRunKey;
  state: ExecutionPhase;
  eventSeq: string;
  elapsedMs: number;
  current: boolean;
  physical: ExecutionPhysical;
  pollAfterMs: number;
  result?: ExecutionResult;
}
export interface ExecutionReceipt { state: 'accepted'; replayed: boolean; key: ExecutionRunKey }
export interface ExecutionReply<T> { state: ExecutionWireState; result: T }
export interface ExecutionSession {
  id: string; resourceDomain: string;
  resourcePolicy: Record<string, number | null>;
  state: ExecutionWireState;
}
export interface ExecutionStart {
  startKey: string; expectedPlanVersion: string; targets: Endpoint[];
  input?: RuntimeInput; runBudgetMs?: number | null;
  jobBudgets?: { nodeId: string; budgetMs: number | null }[];
}
export function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
export function exact(value: unknown, required: string[], optional: string[] = []): value is Record<string, unknown> {
  return object(value) && required.every(key => Object.hasOwn(value, key))
    && Object.keys(value).every(key => required.includes(key) || optional.includes(key));
}
export function decimal(value: unknown, zero = false): value is string {
  return typeof value === 'string' && value.length <= 20
    && (zero && value === '0' || /^[1-9][0-9]*$/.test(value)) && BigInt(value) <= 18446744073709551615n;
}
export function epoch(value: unknown): value is string { return typeof value === 'string' && /^[0-9a-f]{32}$/.test(value); }
export function safeInteger(value: unknown, positive = false): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= (positive ? 1 : 0);
}
export function wireState(value: unknown): value is ExecutionWireState {
  return exact(value, ['revision', 'activePlanVersion', 'preparedToken', 'canRollback', 'latestInputRevision', 'deliveryGeneration'])
    && decimal(value.revision, true) && (value.activePlanVersion === null || decimal(value.activePlanVersion))
    && (value.preparedToken === null || typeof value.preparedToken === 'string' && value.preparedToken.length > 0)
    && typeof value.canRollback === 'boolean' && (value.latestInputRevision === null || safeInteger(value.latestInputRevision, true))
    && decimal(value.deliveryGeneration);
}
export function runKey(value: unknown): value is ExecutionRunKey {
  return exact(value, ['sessionEpoch', 'planVersion', 'runId', 'deliveryGeneration'], ['inputRevision'])
    && epoch(value.sessionEpoch) && decimal(value.planVersion) && decimal(value.runId) && decimal(value.deliveryGeneration)
    && (!Object.hasOwn(value, 'inputRevision') || safeInteger(value.inputRevision, true));
}
export function sameRun(a: ExecutionRunKey, b: ExecutionRunKey) {
  return a.sessionEpoch === b.sessionEpoch && a.planVersion === b.planVersion && a.runId === b.runId
    && a.inputRevision === b.inputRevision && a.deliveryGeneration === b.deliveryGeneration;
}
export function errorValue(value: unknown): value is { code: string; message: string } {
  return exact(value, ['code', 'message']) && typeof value.code === 'string' && typeof value.message === 'string';
}
function endpoint(value: unknown): value is Endpoint {
  return exact(value, ['node', 'port']) && typeof value.node === 'string' && value.node.length > 0 && typeof value.port === 'string' && value.port.length > 0;
}
function terminalResult(value: unknown): value is ExecutionResult {
  if (!exact(value, ['reason', 'delivery', 'targets', 'ports', 'nodes', 'causes', 'recoveries'], ['error'])
    || !Array.isArray(value.targets) || !value.targets.every(endpoint) || !Array.isArray(value.ports)
    || !value.ports.every(port => exact(port, ['endpoint', 'value']) && endpoint(port.endpoint))
    || !Array.isArray(value.nodes) || !Array.isArray(value.causes) || !Array.isArray(value.recoveries)
    || Object.hasOwn(value, 'error') && !errorValue(value.error)) return false;
  if (value.reason === 'completed') {
    return !Object.hasOwn(value, 'error') && typeof value.delivery === 'string' && ['complete', 'recovered', 'no-output', 'failed'].includes(value.delivery)
      && (value.delivery !== 'recovered' || value.recoveries.length > 0);
  }
  return typeof value.reason === 'string' && ['cancelled', 'expired', 'failed'].includes(value.reason) && value.delivery === value.reason;
}
export function runSnapshot(value: unknown): value is ExecutionSnapshot {
  return exact(value, ['key', 'state', 'eventSeq', 'elapsedMs', 'current', 'physical', 'pollAfterMs'], ['result'])
    && runKey(value.key) && typeof value.state === 'string' && ['accepted', 'running', 'terminal'].includes(value.state)
    && decimal(value.eventSeq) && safeInteger(value.elapsedMs) && typeof value.current === 'boolean'
    && exact(value.physical, ['running', 'cancelRequested', 'stalled', 'queued']) && Object.values(value.physical).every(v => safeInteger(v))
    && safeInteger(value.pollAfterMs)
    && (value.state === 'terminal' ? terminalResult(value.result) : !Object.hasOwn(value, 'result'));
}
