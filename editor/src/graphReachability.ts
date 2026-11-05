import { type Definition, type Graph, type GraphEdge } from './model';
export interface ConnectionReachability {
  source: string;
  reached: { node: string; path: GraphEdge[] }[];
  issues: string[];
  connectionsResolved: boolean;
  risk: 'not-assessed';
}
const endpointKey = (node: string, port: string) => JSON.stringify([node, port]);
/** Iterative Kosaraju on all raw edges, including invalid ports, matching the
 * cycle test in connectionError without recursion or per-edge full traversals. */
function components(graph: Graph): Map<string, number> {
  const adjacent = new Map<string, string[]>(), reverse = new Map<string, string[]>();
  const add = (id: string) => { if (!adjacent.has(id)) { adjacent.set(id, []); reverse.set(id, []); } };
  for (const node of graph.nodes) add(node.id);
  for (const edge of graph.edges) { add(edge.from.node); add(edge.to.node); adjacent.get(edge.from.node)!.push(edge.to.node); reverse.get(edge.to.node)!.push(edge.from.node); }
  const visited = new Set<string>(), order: string[] = [];
  for (const root of adjacent.keys()) {
    if (visited.has(root)) continue;
    visited.add(root); const stack: [string, number][] = [[root, 0]];
    while (stack.length) {
      const frame = stack[stack.length - 1], children = adjacent.get(frame[0])!;
      if (frame[1] < children.length) {
        const child = children[frame[1]++];
        if (!visited.has(child)) { visited.add(child); stack.push([child, 0]); }
      } else { order.push(frame[0]); stack.pop(); }
    }
  }
  const result = new Map<string, number>(); let component = 0;
  for (let i = order.length - 1; i >= 0; i--) {
    const root = order[i]; if (result.has(root)) continue;
    const pending = [root]; result.set(root, component);
    while (pending.length) for (const parent of reverse.get(pending.pop()!)!) {
      if (!result.has(parent)) { result.set(parent, component); pending.push(parent); }
    }
    component++;
  }
  return result;
}
/** Conservative may-reach: every input may affect every output. Indexed
 * structure checks + iterative SCC + BFS; path arrays materialize only on use.
 * No RunContext/OS access or external risk is assessed. */
export function traceConnections(graph: Graph, definitions: Definition[], source: string): ConnectionReachability {
  const issues = new Set<string>();
  const nodes = new Map(graph.nodes.map(node => [node.id, node]));
  if (nodes.size !== graph.nodes.length) issues.add('ノードIDが重複しています。');
  if (graph.version !== 1) issues.add('グラフの版を解決できません。');
  if (!nodes.has(source)) issues.add('解析元のノードがありません。');
  const catalog = new Map<string, Definition>();
  for (const definition of definitions) { const key = JSON.stringify([definition.kind, definition.version]); if (!catalog.has(key)) catalog.set(key, definition); }
  const ports = new Map<string, { outputs: Map<string, Definition['outputs'][number]>; inputs: Map<string, Definition['inputs'][number]['port']> }>();
  for (const node of graph.nodes) {
    const definition = catalog.get(JSON.stringify([node.kind, node.version]));
    if (!definition) issues.add(`${node.id}: ノード宣言を取得できません。`);
    if (!ports.has(node.id)) ports.set(node.id, {
      outputs: new Map([...(definition?.outputs ?? [])].reverse().map(port => [port.id, port])),
      inputs: new Map([...(definition?.inputs ?? [])].reverse().map(input => [input.port.id, input.port])),
    });
  }
  const counts = new Map<string, number>();
  for (const edge of graph.edges) { const key = endpointKey(edge.to.node, edge.to.port); counts.set(key, (counts.get(key) ?? 0) + 1); }
  const component = components(graph);
  // Preserve connectionError's error precedence and text; a generated oracle
  // test compares both successful paths and all issues against that API.
  const valid = graph.edges.flatMap(edge => {
    const output = ports.get(edge.from.node)?.outputs.get(edge.from.port), input = ports.get(edge.to.node)?.inputs.get(edge.to.port);
    const error = !output || !input ? '存在する出力・入力ポートを選んでください。'
      : counts.get(endpointKey(edge.to.node, edge.to.port))! > 1 ? 'この入力にはすでに接続があります。先に既存の接続を削除してください。'
      : output.dataType.id !== input.dataType.id || output.dataType.version !== input.dataType.version || (input.language !== null && input.language !== output.language) || (input.root !== 'any' && input.root !== output.root) ? 'ポートのデータ型・言語・ルート型が一致しません。'
      : component.get(edge.from.node) === component.get(edge.to.node) ? '循環する接続は作れません。' : null;
    if (error) { issues.add(`${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}: ${error}`); return []; }
    return [{ edge, key: JSON.stringify([edge.from.node, edge.from.port, edge.to.node, edge.to.port]) }];
  }).toSorted((a, b) => a.key < b.key ? -1 : a.key > b.key ? 1 : 0);
  const adjacent = new Map<string, GraphEdge[]>();
  for (const { edge } of valid) { const list = adjacent.get(edge.from.node) ?? []; list.push(edge); adjacent.set(edge.from.node, list); }
  const reached: ConnectionReachability['reached'] = [];
  if (nodes.size === graph.nodes.length && nodes.has(source) && graph.version === 1) {
    const seen = new Set([source]), parents = new Map<string, { previous: string; edge: GraphEdge }>(), pending = [source];
    for (let i = 0; i < pending.length; i++) for (const edge of adjacent.get(pending[i]) ?? []) {
      const node = edge.to.node; if (seen.has(node)) continue;
      seen.add(node); parents.set(node, { previous: pending[i], edge }); pending.push(node);
      let cached: GraphEdge[] | undefined;
      reached.push({ node, get path() {
        if (!cached) { cached = []; let current = node; while (parents.has(current)) { const link = parents.get(current)!; cached.push(link.edge); current = link.previous; } cached.reverse(); }
        return cached;
      } });
    }
  }
  return { source, reached, issues: [...issues], connectionsResolved: issues.size === 0, risk: 'not-assessed' };
}
