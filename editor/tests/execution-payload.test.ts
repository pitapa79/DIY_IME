import { expect, it } from 'vitest';
import { decodeExecution, executionDefinitions, packet } from '../src/executionPayload';
import type { ExecutionResult, ExecutionRunKey } from '../src/executionModel';
const key: ExecutionRunKey = { sessionEpoch: '0123456789abcdef0123456789abcdef', planVersion: '9007199254740993', runId: '18446744073709551615', inputRevision: 8, deliveryGeneration: '5' };
const port = { id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'any' };
const definition = { kind: 'input.text', version: 1, inputs: [], outputs: [port], configSchema: {}, defaultConfig: {} };
const result = (): ExecutionResult => ({ reason: 'completed', delivery: 'complete', targets: [{ node: 'source', port: 'out' }], ports: [{ endpoint: { node: 'source', port: 'out' }, value: { status: 'data', packet: { version: 1, root: { type: 'text', text: 'かな😀' } } } }], nodes: [{ nodeId: 'source', status: 'executed', diagnostic: null }], causes: [], recoveries: [] });
it('accepts explicit v3 definitions only and retains capability declarations', () => {
  const wrapper = { version: 2, executionKind: 'sync', definition, errorInputs: [] };expect(executionDefinitions([wrapper])).toBe(true);expect(executionDefinitions([definition])).toBe(false);expect(executionDefinitions([wrapper, wrapper])).toBe(false);
  for (const edit of [{ version: 1 }, { executionKind: 'unknown' }, { errorInputs: null }, { definition: { ...definition, outputs: [{ ...port, root: ['any'] }] } }]) expect(executionDefinitions([{ ...wrapper, ...edit }])).toBe(false);
});
it('validates nested Packet v1 without choosing Choice or treating empty Data as NoOutput', () => {
  expect(packet({ version: 1, root: { type: 'choice', options: [{ value: { type: 'sequence', items: [{ type: 'text', text: '', meta: { reading: '', sourceSpan: [0, 10] } }] }, score: -1 }] } })).toBe(true);
  for (const root of [{ type: 'text', text: '\ud800' }, { type: 'text', text: 'ok', meta: { reading: '\udfff' } }, { type: 'text', text: 1 }, { type: 'choice', options: [{ value: { type: 'text', text: 'x' }, score: Infinity }] }, { type: 'text', text: 'x', meta: { sourceSpan: [2, 1] } }]) expect(packet({ version: 1, root })).toBe(false);
  let root: unknown = { type: 'text', text: 'deep' };for (let i = 0; i < 2000; i++) root = { type: 'sequence', items: [root] };expect(packet({ version: 1, root })).toBe(true);
});
it('rejects foreign cause identity and invalid output atomically while preserving recovery and diagnostics', () => {
  const id = { sessionEpoch: key.sessionEpoch, planVersion: key.planVersion, runId: key.runId, nodeId: 'source' };
  const recovered: ExecutionResult = { ...result(), delivery: 'recovered', causes: [{ id, diagnostic: { ...id, port: 'out', error: { code: 'original', message: 'failed upstream' } }, paths: [] }], recoveries: [{ nodeId: 'recover', input: 'main', output: 'out', causes: [id] }] };
  const decoded = decodeExecution(recovered, key)!;expect(decoded.causes[0].id.runId).toBe('18446744073709551615');expect(decoded.recoveries[0].causes).toEqual([id]);
  expect(decodeExecution({ ...recovered, causes: [{ ...recovered.causes[0] as object, id: { ...id, runId: '1' } }] }, key)).toBeNull();
  expect(decodeExecution({ ...recovered, ports: [...recovered.ports, { endpoint: { node: 'bad', port: 'out' }, value: { status: 'data', packet: { version: 2 } } }] }, key)).toBeNull();
  expect(decodeExecution({ ...recovered, ports: [...recovered.ports, ...recovered.ports] }, key)).toBeNull();
  expect(decodeExecution({ ...recovered, recoveries: [{ nodeId: 'recover', input: 'main', output: 'out', causes: [{ ...id, nodeId: 'missing' }] }] }, key)).toBeNull();
});
