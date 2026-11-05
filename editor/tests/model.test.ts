import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { runtimeInputError, runtimeInputKey } from '../src/model';
import { commit, connectionError, graphKey, initialDocument, layout, newNodeId, parseGraph, redo, removeNode, undo, type Definition, type Graph } from '../src/model';

const hello = parseGraph(readFileSync(new URL('../../examples/graphs/hello.json', import.meta.url), 'utf8'));
const port = { id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'any' as const };
const definitions: Definition[] = [
  { kind: 'demo.packet', version: 1, inputs: [], outputs: [port], configSchema: {}, defaultConfig: {} },
  ...['demo.uppercase', 'demo.select-text'].map(kind => ({ kind, version: 1, inputs: [{ port: { ...port, id: 'in' }, required: true }], outputs: [port], configSchema: {}, defaultConfig: {} })),
];
describe('graph files and editing', () => {
  it('rejects isolated UTF-16 surrogates while preserving valid Unicode input', () => {
    for (const text of ['\ud800', '\udfff', 'a\ud800b', '\udc00\ud800', '\ud800\ud800']) {
      expect(runtimeInputError({ revision: 1, text, mode: 'compose' })).toContain('読み取れない文字');
    }
    for (const text of ['', '\ud800\udc00', '\udbff\udfff', '𠮷か\u3099👩‍💻\ufffd', '𠮷'.repeat(4096)]) {
      expect(runtimeInputError({ revision: 1, text, mode: 'compose' })).toBeUndefined();
    }
  });
  it('validates runtime revisions and Unicode code points and keys mode changes', () => {
    const input = { revision: Number.MAX_SAFE_INTEGER, text: '𠮷'.repeat(4096), mode: 'compose' as const };
    expect(runtimeInputError(input)).toBeUndefined();
    expect(runtimeInputError({ ...input, text: input.text + 'a' })).toBeTruthy();
    expect(runtimeInputError({ ...input, revision: 0 })).toBeTruthy();
    expect(runtimeInputError({ ...input, revision: Number.MAX_SAFE_INTEGER + 1 })).toBeTruthy();
    expect(runtimeInputKey(input)).not.toBe(runtimeInputKey({ ...input, mode: 'finalize' }));
    expect(runtimeInputError({ revision: 1, text: '', mode: 'compose' })).toBeUndefined();
  });
  it('compares Rust and browser graphs independently of JSON object key order', () => {
    const reordered = JSON.parse(JSON.stringify(hello, (_key, value) => value && typeof value === 'object' && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).reverse()) : value));
    expect(graphKey(reordered)).toBe(graphKey(hello));
  });
  it('round trips the CLI sample without introducing editor properties', () => {
    const doc = initialDocument(hello, 'hello.json');
    expect(parseGraph(JSON.stringify(doc.graph))).toEqual(hello);
    expect(Object.keys(doc.graph)).toEqual(['version', 'nodes', 'edges']);
    expect(doc.positions.input.x).toBeLessThan(doc.positions.uppercase.x);
    expect(doc.positions.uppercase.x).toBeLessThan(doc.positions.result.x);
  });
  it.each([
    '{', '{}', 'null', '{"version":1,"nodes":[],"edges":[],"layout":{}}',
    '{"version":1,"nodes":[{"id":"a","kind":"x","version":1},{"id":"a","kind":"y","version":1}],"edges":[]}',
    '{"version":1,"nodes":[],"edges":[{"from":{"node":"missing","port":"out"},"to":{"node":"missing","port":"in"}}]}',
    '{"version":1,"nodes":[{"id":"a","kind":"x","version":1,"config":null}],"edges":[]}',
  ])('rejects unsafe editor structure without silently repairing it: %s', input => {
    expect(() => parseGraph(input)).toThrow();
  });
  it('rejects identical edges while keeping cyclic graphs editable', () => {
    expect(() => parseGraph(JSON.stringify({ ...hello, edges: [...hello.edges, hello.edges[0]] }))).toThrow('重複');
    const cyclic = { ...hello, edges: [...hello.edges, { from: { node: 'result', port: 'out' }, to: { node: 'uppercase', port: 'in' } }] };
    expect(Object.keys(layout(parseGraph(JSON.stringify(cyclic))))).toHaveLength(3);
  });
  it('removes incident edges and restores the entire document with undo/redo', () => {
    const original = initialDocument(hello);
    const edited = { ...original, graph: removeNode(hello, 'uppercase') };
    expect(edited.graph.edges).toHaveLength(0);
    const history = commit({ past: [], present: original, future: [] }, edited);
    expect(undo(history).present).toEqual(original);
    expect(redo(undo(history)).present).toEqual(edited);
    expect(undo({ past: [], present: original, future: [] }).present).toBe(original);
  });
  it('creates unique IDs and handles punctuation in IDs without edge collisions', () => {
    const graph: Graph = { ...hello, nodes: [...hello.nodes, { id: 'identity_1', kind: 'demo.identity', version: 1 }] };
    expect(newNodeId(graph, 'demo.identity')).toBe('identity_2');
    const special = parseGraph('{"version":1,"nodes":[{"id":"__proto__","kind":"demo.packet","version":1}],"edges":[]}');
    expect(Object.hasOwn(layout(special), '__proto__')).toBe(true);
  });
});
describe('connection checks', () => {
  const withoutEdges = { ...hello, edges: [] };
  it('allows compatible connections and rejects occupied inputs and cycles', () => {
    expect(connectionError(withoutEdges, definitions, hello.edges[0])).toBeNull();
    expect(connectionError(hello, definitions, hello.edges[0])).toContain('すでに接続');
    const backward = { from: { node: 'result', port: 'out' }, to: { node: 'uppercase', port: 'in' } };
    expect(connectionError({ ...hello, edges: [hello.edges[1]] }, definitions, backward)).toContain('循環');
    expect(connectionError(withoutEdges, definitions, { from: { node: 'input', port: 'missing' }, to: { node: 'uppercase', port: 'in' } })).toContain('存在');
  });
  it('requires a guaranteed language and root type for constrained inputs', () => {
    const constrained = structuredClone(definitions);
    constrained[1].inputs[0].port.language = 'ja';
    expect(connectionError(withoutEdges, constrained, hello.edges[0])).toContain('一致');
    constrained[0].outputs[0].language = 'ja';
    expect(connectionError(withoutEdges, constrained, hello.edges[0])).toBeNull();
    constrained[1].inputs[0].port.root = 'text';
    expect(connectionError(withoutEdges, constrained, hello.edges[0])).toContain('一致');
    constrained[0].outputs[0].root = 'text';
    expect(connectionError(withoutEdges, constrained, hello.edges[0])).toBeNull();
  });
});
