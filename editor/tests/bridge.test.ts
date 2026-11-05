import { resolve } from 'node:path';
import { createServer, type Server } from 'node:http';
import { readFileSync } from 'node:fs';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createApiMiddleware, invokeCore } from '../server/bridge';

const sample = readFileSync(new URL('../../examples/graphs/hello.json', import.meta.url), 'utf8');
let server: Server, origin: string;
beforeAll(async () => {
  const middleware = createApiMiddleware({ legacy: { core: resolve(process.cwd(), '../target/debug', process.platform === 'win32' ? 'diy-ime2-test-host.exe' : 'diy-ime2-test-host'), root: resolve(process.cwd(), '..') } });
  server = createServer((request, response) => middleware(request, response, () => { response.writeHead(404); response.end(); }));
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('missing address');
  origin = `http://127.0.0.1:${address.port}`;
});
afterAll(async () => { server.closeAllConnections(); await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve())); });
async function post(path: string, body: string, headers: Record<string, string> = {}) {
  return fetch(origin + path, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers }, body });
}
describe('real Rust CLI integration', () => {
  it('loads definitions and samples from the repository', async () => {
    const response = await fetch(origin + '/api/nodes');
    expect(response.status).toBe(200);
    const nodes = await response.json();
    expect(nodes.data).toHaveLength(9);
    expect(nodes.data.some((node: { kind: string }) => node.kind === 'demo.packet')).toBe(true);
    const example = await fetch(origin + '/api/examples/hello').then(response => response.json());
    expect(example.data).toEqual(JSON.parse(sample));
  });
  it('validates and executes a graph in Rust', async () => {
    const checked = await post('/api/graph/validate', sample).then(response => response.json());
    expect(checked).toEqual({ ok: true, data: { valid: true, order: ['input', 'uppercase', 'result'] } });
    const result = await post('/api/graph/run', sample).then(response => response.json());
    expect(result.ok).toBe(true); expect(result.data.succeeded).toBe(true);
    expect(result.data.nodes.result.outputs.out.packet.root.text).toBe('HELLO 𠮷');
  });
  it('serves the dictionary graph without embedding dictionary bytes or granting a root', async () => {
    const response = await fetch(origin + '/api/examples/dictionary-exact');
    expect(response.status).toBe(200);
    const { data: graph } = await response.json();
    expect(graph.nodes).toHaveLength(5);
    expect(graph.nodes.find((node: { id: string }) => node.id === 'dictionary')).toEqual({ id: 'dictionary', kind: 'text.dictionary-exact', version: 1, config: { path: 'dictionary.tsv' } });
    expect(graph.nodes[0].kind).toBe('input.text');
  });


  it('returns Rust connection errors without substituting browser execution', async () => {
    const graph = JSON.parse(sample); graph.edges[0].from.port = 'missing';
    const response = await post('/api/graph/run', JSON.stringify(graph));
    expect(response.status).toBe(422);
    const error = await response.json();
    expect(error.ok).toBe(false); expect(error.error.code).toBe('unknown-output');
    expect(error.error.message).toContain('input.missing');
  });
  it('returns settings errors from Rust and contains malformed requests', async () => {
    const graph = JSON.parse(sample); graph.nodes[0].config.packet.version = 2;
    const response = await post('/api/graph/validate', JSON.stringify(graph));
    expect((await response.json()).error.code).toBe('invalid-config');
    expect((await post('/api/graph/run', '{')).status).toBe(422);
    expect((await post('/api/graph/run', '{}', { 'Content-Type': 'text/plain' })).status).toBe(415);
    expect((await post('/api/graph/run', ' '.repeat(1024 * 1024 + 1))).status).toBe(413);
  });
  it('rejects cross-origin calls and unknown routes', async () => {
    expect((await post('/api/graph/run', sample, { Origin: 'https://example.com' })).status).toBe(403);
    expect((await fetch(origin + '/api/examples/unknown')).status).toBe(404);
    expect((await fetch(origin + '/api/graph/run')).status).toBe(404);
  });
  it('can cancel execution requests', async () => {
    const controller = new AbortController(); controller.abort();
    await expect(invokeCore('run', sample, controller.signal)).rejects.toThrow('取り消し');
  });
});
