import type { Definition, Endpoint, Packet, Port } from './model';
import { errorValue, exact, object, safeInteger, type ExecutionRunKey, type ExecutionResult } from './executionModel';
export interface ExecutionDefinition { version: 2; executionKind: 'sync' | 'deferred' | 'control'; definition: Definition; errorInputs: string[] }
const unicode = (value: unknown): value is string => typeof value === 'string' && !/[\uD800-\uDFFF]/u.test(value);
const text = (value: unknown): value is string => typeof value === 'string' && value.length > 0;
function port(value: unknown): value is Port {
  return exact(value, ['id', 'dataType', 'language', 'root']) && text(value.id)
    && exact(value.dataType, ['id', 'version']) && text(value.dataType.id) && safeInteger(value.dataType.version, true)
    && (value.language === null || typeof value.language === 'string') && typeof value.root === 'string' && ['any', 'text', 'sequence', 'choice'].includes(value.root);
}
export function executionDefinitions(value: unknown): value is ExecutionDefinition[] {
  return Array.isArray(value) && value.every(item => exact(item, ['version', 'executionKind', 'definition', 'errorInputs'])
    && item.version === 2 && typeof item.executionKind === 'string' && ['sync', 'deferred', 'control'].includes(item.executionKind)
    && Array.isArray(item.errorInputs) && item.errorInputs.every(text)
    && exact(item.definition, ['kind', 'version', 'inputs', 'outputs', 'configSchema', 'defaultConfig'])
    && text(item.definition.kind) && safeInteger(item.definition.version, true)
    && object(item.definition.configSchema)
    && Array.isArray(item.definition.inputs) && item.definition.inputs.every(input => exact(input, ['port', 'required']) && port(input.port) && typeof input.required === 'boolean')
    && Array.isArray(item.definition.outputs) && item.definition.outputs.every(port))
    && new Set(value.map(item => JSON.stringify([item.definition.kind, item.definition.version]))).size === value.length;
}
export function packet(value: unknown): value is Packet {
  if (!exact(value, ['version', 'root']) || value.version !== 1) return false;
  const pending: unknown[] = [value.root], seen = new WeakSet<object>();
  while (pending.length) {
    const node = pending.pop();if (node === null || typeof node !== 'object' || seen.has(node)) return false;seen.add(node);
    if (exact(node, ['type', 'text'], ['meta']) && node.type === 'text') {
      if (!unicode(node.text)) return false;
      if (Object.hasOwn(node, 'meta')) {
        if (!exact(node.meta, [], ['reading', 'sourceSpan']) || Object.hasOwn(node.meta, 'reading') && !unicode(node.meta.reading)) return false;
        const span = node.meta.sourceSpan;
        if (span !== undefined && (!Array.isArray(span) || span.length !== 2 || !span.every(v => safeInteger(v)) || span[0] > span[1])) return false;
      }
    } else if (exact(node, ['type', 'items']) && node.type === 'sequence' && Array.isArray(node.items)) { for (const item of node.items) pending.push(item); }
    else if (exact(node, ['type', 'options']) && node.type === 'choice' && Array.isArray(node.options)) {
      for (const option of node.options) {
        if (!exact(option, ['value'], ['score']) || Object.hasOwn(option, 'score') && (typeof option.score !== 'number' || !Number.isFinite(option.score))) return false;
        pending.push(option.value);
      }
    } else return false;
  }
  return true;
}
interface CauseId { sessionEpoch: string; planVersion: string; runId: string; nodeId: string }
export interface ExecutionDiagnostic extends CauseId { port: string | null; error: { code: string; message: string } }
export interface ExecutionCause { id: CauseId; diagnostic: ExecutionDiagnostic; paths: unknown[] }
export interface ExecutionRecovery { nodeId: string; input: string; output: string; causes: CauseId[] }
export type ExecutionPort = { status: 'data'; packet: Packet } | { status: 'no-output' }
  | { status: 'error'; causes: CauseId[] } | { status: 'discarded'; reason: string };
export interface DecodedExecution {
  ports: { endpoint: Endpoint; value: ExecutionPort }[];
  nodes: { nodeId: string; status: 'executed' | 'skipped' | 'failed' | 'blocked' | 'discarded'; diagnostic: ExecutionDiagnostic | null }[];
  causes: ExecutionCause[]; recoveries: ExecutionRecovery[];
}
function identity(value: Record<string, unknown>, key: ExecutionRunKey) {
  return value.sessionEpoch === key.sessionEpoch && value.planVersion === key.planVersion && value.runId === key.runId && text(value.nodeId);
}
function causeId(value: unknown, key: ExecutionRunKey): value is CauseId {
  return exact(value, ['sessionEpoch', 'planVersion', 'runId', 'nodeId']) && identity(value, key);
}
function diagnostic(value: unknown, key: ExecutionRunKey): value is ExecutionDiagnostic {
  return exact(value, ['sessionEpoch', 'planVersion', 'runId', 'nodeId', 'port', 'error']) && identity(value, key)
    && (value.port === null || text(value.port)) && errorValue(value.error);
}
const causeKey = (id: CauseId) => JSON.stringify([id.sessionEpoch, id.planVersion, id.runId, id.nodeId]);
/** Decode all output/diagnostic payloads atomically: no partial Data on failure. */
export function decodeExecution(result: ExecutionResult, key: ExecutionRunKey): DecodedExecution | null {
  const ids = new Set<string>();
  const causes: ExecutionCause[] = [];
  for (const value of result.causes) {
    if (!exact(value, ['id', 'diagnostic', 'paths']) || !causeId(value.id, key) || !diagnostic(value.diagnostic, key)
      || value.id.nodeId !== value.diagnostic.nodeId || !Array.isArray(value.paths) || ids.has(causeKey(value.id))) return null;
    ids.add(causeKey(value.id));causes.push({ id: value.id, diagnostic: value.diagnostic, paths: value.paths });
  }
  const knownCause = (value: unknown): value is CauseId => causeId(value, key) && ids.has(causeKey(value));
  const ports: DecodedExecution['ports'] = [], endpoints = new Set<string>();
  for (const item of result.ports) {
    const id = JSON.stringify([item.endpoint.node, item.endpoint.port]);if (endpoints.has(id)) return null;endpoints.add(id);
    const value = item.value;
    if (exact(value, ['status', 'packet']) && value.status === 'data' && packet(value.packet)) ports.push({ endpoint: item.endpoint, value: { status: 'data', packet: value.packet } });
    else if (exact(value, ['status']) && value.status === 'no-output') ports.push({ endpoint: item.endpoint, value: { status: 'no-output' } });
    else if (exact(value, ['status', 'causes']) && value.status === 'error' && Array.isArray(value.causes) && value.causes.length > 0 && value.causes.every(knownCause)) ports.push({ endpoint: item.endpoint, value: { status: 'error', causes: value.causes } });
    else if (exact(value, ['status', 'reason']) && value.status === 'discarded' && typeof value.reason === 'string') ports.push({ endpoint: item.endpoint, value: { status: 'discarded', reason: value.reason } });
    else return null;
  }
  const nodes: DecodedExecution['nodes'] = [], nodeIds = new Set<string>();
  for (const value of result.nodes) {
    if (!exact(value, ['nodeId', 'status', 'diagnostic']) || !text(value.nodeId) || nodeIds.has(value.nodeId) || typeof value.status !== 'string'
      || !['executed', 'skipped', 'failed', 'blocked', 'discarded'].includes(value.status)
      || value.diagnostic !== null && (!diagnostic(value.diagnostic, key) || value.diagnostic.nodeId !== value.nodeId)) return null;
    nodeIds.add(value.nodeId);nodes.push(value as unknown as DecodedExecution['nodes'][number]);
  }
  const recoveries: ExecutionRecovery[] = [];
  for (const value of result.recoveries) {
    if (!exact(value, ['nodeId', 'input', 'output', 'causes']) || !text(value.nodeId) || !text(value.input) || !text(value.output)
      || !Array.isArray(value.causes) || value.causes.length === 0 || !value.causes.every(knownCause)) return null;
    recoveries.push({ nodeId: value.nodeId, input: value.input, output: value.output, causes: value.causes });
  }
  return { ports, nodes, causes, recoveries };
}
export function jsonPreview(value: unknown) { try { return JSON.stringify(value, null, 2); } catch { return '深いデータのため、この画面ではJSONを展開できません。'; } }
