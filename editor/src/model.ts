import { experimentalMetadata } from './experimentalNodes';
export type Candidate =
  | { type: 'text'; text: string; meta?: { reading?: string; sourceSpan?: [number, number] } }
  | { type: 'sequence'; items: Candidate[] }
  | { type: 'choice'; options: { value: Candidate; score?: number }[] };
export interface Packet { version: number; root: Candidate }
export interface GraphNode { id: string; kind: string; version: number; config?: unknown }
export interface Endpoint { node: string; port: string }
export interface GraphEdge { from: Endpoint; to: Endpoint }
export interface Graph { version: number; nodes: GraphNode[]; edges: GraphEdge[] }
export interface Port {
  id: string; dataType: { id: string; version: number };
  language: string | null; root: 'any' | 'text' | 'sequence' | 'choice';
}
export interface Definition {
  kind: string; version: number; inputs: { port: Port; required: boolean }[]; outputs: Port[];
  configSchema: { properties?: Record<string, { type?: string | string[]; enum?: string[] }> }; defaultConfig: unknown;
}
export interface Diagnostic { nodeId: string; port: string | null; runId: number; planVersion: number; error: { code: string; message: string } }
export type PortResult = { status: 'data'; packet: Packet } | { status: 'no-output' } | { status: 'error'; diagnostic: Diagnostic };
export interface NodeResult { status: 'executed' | 'skipped' | 'failed' | 'blocked'; outputs: Record<string, PortResult>; diagnostic: Diagnostic | null }
// Canvas state is separate from the legacy v2 report contract.
export type NodeDisplayStatus = NodeResult['status'] | 'discarded';
export interface Report { succeeded: boolean; runId: number; planVersion: number; inputRevision?: number; nodes: Record<string, NodeResult> }
export interface RuntimeInput { revision: number; text: string; mode: 'compose' | 'finalize' }
export const runtimeInputKey = (input: RuntimeInput) => JSON.stringify([input.revision, input.text, input.mode]);
export const needsRuntimeInput = (graph: Graph) => graph.nodes.some(node => node.kind === 'input.text');
export function runtimeInputError(input: RuntimeInput): string | undefined {
  if (!Number.isSafeInteger(input.revision) || input.revision < 1) return '入力の更新番号が無効です。';
  if (input.mode !== 'compose' && input.mode !== 'finalize') return '入力モードが無効です。';
  if (typeof input.text !== 'string') return '4096文字以内で入力してください。';
  let length = 0;
  for (const character of input.text) {
    const point = character.codePointAt(0)!;
    // JSの文字列には孤立サロゲートも入るが、Rustへ渡せるUnicode文字ではない。
    if (point >= 0xd800 && point <= 0xdfff) return '入力に読み取れない文字が含まれています。入力し直してください。';
    if (++length > 4096) return '4096文字以内で入力してください。';
  }
}
export interface SessionState {
  revision: number;
  active: { version: number; graph: Graph } | null;
  prepared: { token: string; graph: Graph; order: string[] } | null;
  canRollback: boolean;
}
export interface CoreSession { id: string; state: SessionState }
export interface SessionResult<T = unknown> { state: SessionState; result: T }
export type Positions = Record<string, { x: number; y: number }>;
export interface Document { graph: Graph; positions: Positions; name: string }

export const metadata: Record<string, { title: string; description: string; category: string; symbol: string }> = {
  ...experimentalMetadata,
  'text.select-candidates': { title: '候補選択（実用）', description: 'Choiceごとに選び、Sequenceを保持', category: '候補処理', symbol: 'select' },
  'text.dictionary-exact': { title: '完全一致辞書', description: '読み全体から表記候補を出す', category: '変換', symbol: 'kanji' },
  'text.stringify': { title: '文字列化（実用）', description: '選択済みの候補木を順番に連結', category: '候補処理', symbol: 'text' },
  'input.text': { title: 'Input（実行時入力）', description: '計画を変えずに文章を入力する', category: '入出力', symbol: 'input' },
  'text.romaji-to-hiragana': { title: 'ローマ字→ひらがな（実用）', description: '入力途中の末尾を pending に保持', category: '変換', symbol: 'kana' },
  'e2e.input': { title: 'Input（固定文字列）', description: '変換するローマ字を入力する', category: '入出力', symbol: 'input' },
  'e2e.output': { title: 'Output（ログ）', description: '文章を表示・ログ出力する', category: '入出力', symbol: 'output' },
  'e2e.romaji-to-hiragana': { title: 'ローマ字→ひらがな', description: '数字・記号を残して読みへ変換', category: '変換', symbol: 'kana' },
  'e2e.kana-kanji': { title: 'かな漢字候補生成', description: '小辞書から複数の表記を生成', category: '変換', symbol: 'kanji' },
  'e2e.segment': { title: '分割候補生成', description: '区切り方の異なる候補列を生成', category: '候補処理', symbol: 'segment' },
  'e2e.select-candidates': { title: '候補選択', description: '各 Choice のスコアで選ぶ', category: '候補処理', symbol: 'select' },
  'e2e.stringify': { title: '文字列化', description: '選択済みの候補列を文章へ結合', category: '候補処理', symbol: 'text' },
  'demo.packet': { title: '候補データ', description: '文字列や候補木を入力する', category: '互換', symbol: 'input' },
  'demo.identity': { title: 'そのまま渡す', description: '候補データを変更せず渡す', category: '互換', symbol: 'pass' },
  'demo.uppercase': { title: '大文字に変換', description: '候補木の英字を大文字にする', category: '互換', symbol: 'upper' },
  'demo.select-text': { title: '候補を文章に', description: 'スコアで選び、文章にまとめる', category: '互換', symbol: 'select' },
  'demo.branch': { title: '条件分岐', description: 'yes / no の経路を選ぶ', category: '互換', symbol: 'branch' },
};
export const libraryCategories = ['入出力', '変換', '候補処理', '実験'];
export const label = (kind: string) => metadata[kind]?.title ?? kind;
export const definitionFor = (definitions: Definition[], node: GraphNode) => definitions.find(d => d.kind === node.kind && d.version === node.version);
export const edgeId = (edge: GraphEdge) => JSON.stringify([edge.from.node, edge.from.port, edge.to.node, edge.to.port]);
// Rust の Map と JSON ファイルのキー順が違っても、同じグラフとして照合する。
export const graphKey = (graph: Graph) => JSON.stringify(graph, (_key, value) =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, value[key]])) : value);

function object(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === 'object' && !Array.isArray(value); }
function keys(value: Record<string, unknown>, allowed: string[]) { return Object.keys(value).every(key => allowed.includes(key)); }
function identifier(value: unknown): value is string { return typeof value === 'string' && value.trim().length > 0; }

/** 画面で安全に扱うための構造検査。ポート・設定・循環の最終検査は Rust が行う。 */
export function parseGraph(text: string): Graph {
  const value: unknown = JSON.parse(text);
  if (!object(value) || !keys(value, ['version', 'nodes', 'edges']) || !Number.isInteger(value.version)
    || !Array.isArray(value.nodes) || !Array.isArray(value.edges)) throw new Error('version・nodes・edges を持つグラフ JSON が必要です。');
  const ids = new Set<string>();
  for (const node of value.nodes) {
    if (!object(node) || !keys(node, ['id', 'kind', 'version', 'config']) || !identifier(node.id) || !identifier(node.kind)
      || !Number.isInteger(node.version) || node.config === null || ids.has(node.id)) throw new Error('ノードの ID・種別・バージョンを確認してください。ID は重複できません。');
    ids.add(node.id);
  }
  const edgeIds = new Set<string>();
  for (const edge of value.edges) {
    if (!object(edge) || !keys(edge, ['from', 'to'])) throw new Error('接続には from と to が必要です。');
    for (const endpoint of [edge.from, edge.to]) {
      if (!object(endpoint) || !keys(endpoint, ['node', 'port']) || !identifier(endpoint.node) || !identifier(endpoint.port)
        || !ids.has(endpoint.node)) throw new Error('接続先のノード ID とポートを確認してください。');
    }
    const id = edgeId(edge as unknown as GraphEdge);
    if (edgeIds.has(id)) throw new Error('同じ接続が重複しています。');
    edgeIds.add(id);
  }
  return value as unknown as Graph;
}

export function layout(graph: Graph): Positions {
  const levels = new Map<string, number>();
  const incoming = new Map(graph.nodes.map(node => [node.id, 0]));
  const outgoing = new Map(graph.nodes.map(node => [node.id, new Set<string>()]));
  for (const edge of graph.edges) {
    if (outgoing.has(edge.from.node) && incoming.has(edge.to.node) && !outgoing.get(edge.from.node)!.has(edge.to.node)) {
      outgoing.get(edge.from.node)!.add(edge.to.node); incoming.set(edge.to.node, incoming.get(edge.to.node)! + 1);
    }
  }
  const ready = graph.nodes.filter(node => incoming.get(node.id) === 0).map(node => node.id);
  for (let i = 0; i < ready.length; i++) {
    const id = ready[i];
    for (const next of outgoing.get(id) ?? []) {
      levels.set(next, Math.max(levels.get(next) ?? 0, (levels.get(id) ?? 0) + 1));
      incoming.set(next, incoming.get(next)! - 1);
      if (incoming.get(next) === 0) ready.push(next);
    }
  }
  const rows = new Map<number, number>();
  return Object.fromEntries(graph.nodes.map(node => {
    const column = levels.get(node.id) ?? 0; const row = rows.get(column) ?? 0; rows.set(column, row + 1);
    return [node.id, { x: column * 330 + 40, y: row * 200 + 90 }];
  }));
}

export function connectionError(graph: Graph, definitions: Definition[], edge: GraphEdge): string | null {
  const source = graph.nodes.find(node => node.id === edge.from.node);
  const target = graph.nodes.find(node => node.id === edge.to.node);
  const output = source && definitionFor(definitions, source)?.outputs.find(port => port.id === edge.from.port);
  const input = target && definitionFor(definitions, target)?.inputs.find(input => input.port.id === edge.to.port)?.port;
  if (!output || !input) return '存在する出力・入力ポートを選んでください。';
  if (graph.edges.some(current => current.to.node === edge.to.node && current.to.port === edge.to.port)) return 'この入力にはすでに接続があります。先に既存の接続を削除してください。';
  if (output.dataType.id !== input.dataType.id || output.dataType.version !== input.dataType.version
    || (input.language !== null && input.language !== output.language) || (input.root !== 'any' && input.root !== output.root)) return 'ポートのデータ型・言語・ルート型が一致しません。';
  const pending = [edge.to.node], visited = new Set<string>();
  while (pending.length) {
    const id = pending.pop()!; if (id === edge.from.node) return '循環する接続は作れません。';
    if (visited.has(id)) continue; visited.add(id);
    graph.edges.filter(current => current.from.node === id).forEach(current => pending.push(current.to.node));
  }
  return null;
}

export function removeNode(graph: Graph, id: string): Graph {
  return { ...graph, nodes: graph.nodes.filter(node => node.id !== id), edges: graph.edges.filter(edge => edge.from.node !== id && edge.to.node !== id) };
}
export function newNodeId(graph: Graph, kind: string): string {
  const base = kind.split('.').at(-1)!.replace(/[^a-zA-Z0-9_-]/g, '') || 'node';
  let index = 1; while (graph.nodes.some(node => node.id === `${base}_${index}`)) index++;
  return `${base}_${index}`;
}
export function initialDocument(graph: Graph, name = 'untitled.json'): Document { return { graph, positions: layout(graph), name }; }

export interface History<T> { past: T[]; present: T; future: T[] }
export function commit<T>(history: History<T>, next: T): History<T> {
  return { past: [...history.past.slice(-49), history.present], present: next, future: [] };
}
export function undo<T>(history: History<T>): History<T> {
  if (!history.past.length) return history;
  return { past: history.past.slice(0, -1), present: history.past.at(-1)!, future: [history.present, ...history.future] };
}
export function redo<T>(history: History<T>): History<T> {
  if (!history.future.length) return history;
  return { past: [...history.past, history.present], present: history.future[0], future: history.future.slice(1) };
}
