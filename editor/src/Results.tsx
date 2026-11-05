import { useEffect, useRef, useState } from 'react';
import { AlertCircle, CheckCircle2, ChevronRight, Link2, Terminal, TextCursorInput } from 'lucide-react';
import { edgeId, type Graph, type GraphEdge, type Report } from './model';
import { statusLabels } from './GraphNode';

const portId = (node: string, port: string) => JSON.stringify([node, port]);
interface Props {
  result: Report | null; graph: Graph; connection?: GraphEdge; onClearConnection: () => void;
  stale: boolean; inputStale?: boolean; order: string[] | null; duration?: number; onSelectNode: (id: string) => void;
}

export default function Results({ result, graph, connection, onClearConnection, stale, inputStale, order, duration, onSelectNode }: Props) {
  const ports = result ? Object.entries(result.nodes).flatMap(([node, execution]) => Object.entries(execution.outputs).map(([port, value]) => ({ id: portId(node, port), label: `${node}.${port}`, node, port, value }))) : [];
  const terminalPorts = ports.filter(item => item.value.status === 'data' && !graph.edges.some(edge => edge.from.node === item.node && edge.from.port === item.port));
  // 途中ノードの未接続補助ポートより、最終ノードの出力を先に表示する。
  const terminal = terminalPorts.find(item => !graph.edges.some(edge => edge.from.node === item.node)) ?? terminalPorts[0];
  const [manualSelection, setManualSelection] = useState('');
  useEffect(() => setManualSelection(terminal?.id ?? ports.findLast(item => item.value.status === 'data')?.id ?? ports.at(-1)?.id ?? ''), [result]);
  const selected = connection ? portId(connection.from.node, connection.from.port) : manualSelection;
  const recorded = !connection || graph.edges.some(edge => edgeId(edge) === edgeId(connection));
  const output = recorded ? ports.find(port => port.id === selected)?.value : undefined;
  const diagnostics = result ? Object.values(result.nodes).filter(node => node.status === 'failed').map(node => node.diagnostic!).filter(Boolean) : [];
  const container = useRef<HTMLDivElement>(null);
  const connectionKey = connection && edgeId(connection);
  useEffect(() => { if (container.current) container.current.scrollTop = 0; }, [result, selected, connectionKey]);
  return <div className="results-content" ref={container}>
    {connection && <div className="connection-data" data-testid="connection-data"><Link2 size={14} /><strong>接続データ</strong><code>{connection.from.node}.{connection.from.port} → {connection.to.node}.{connection.to.port}</code></div>}
    {!result ? <div className="results-empty"><span className="result-empty-icon">{order ? <CheckCircle2 size={22} /> : <Terminal size={22} />}</span><div><h3>{connection ? 'この接続はまだ実行されていません' : order ? '接続の検査が完了しました' : '計画を準備しましょう'}</h3><p>{connection ? '「計画を準備」→「切替」→「実行」で、この段階のデータを確認できます。' : order ? `実行順：${order.join(' → ') || 'ノードなし'}` : '「計画を準備」→「切替」の後、稼働計画を実行できます。'}</p></div></div> : <>
      <div className="result-summary"><span className={`tag ${stale ? 'amber' : result.succeeded ? 'green' : 'red'}`}>{inputStale ? '入力変更前の結果' : stale ? '編集前の結果' : result.succeeded ? '実行完了' : '実行エラー'}</span><span className="muted">計画 v{result.planVersion} · run {result.runId}{result.inputRevision !== undefined ? ` · 入力 r${result.inputRevision}` : ''} · {Object.keys(result.nodes).length} ノード{duration !== undefined ? ` · ${Math.round(duration)} ms` : ''}</span>{stale && <span className="stale-note">編集内容・稼働版との違いに注意</span>}</div>
      {diagnostics.map((diagnostic, index) => <div className="result-error" key={index}><AlertCircle size={16} /><div><strong>{diagnostic.nodeId}{diagnostic.port ? `.${diagnostic.port}` : ''}</strong><p>{diagnostic.error.code}: {diagnostic.error.message}</p></div></div>)}
      {(ports.length > 0 || connection) && <div className="output-block"><div className="output-heading"><span><TextCursorInput size={14} />{connection ? '接続元の出力' : '出力'}</span><select aria-label="表示する出力" value={selected} onChange={event => { setManualSelection(event.target.value); onClearConnection(); }}>
        {connection && !ports.some(port => port.id === selected) && <option value={selected}>{connection.from.node}.{connection.from.port}</option>}
        {ports.map(port => <option value={port.id} key={port.id}>{port.label}</option>)}
      </select></div>
        {output?.status === 'data' && <><div className="output-value" data-testid="output-value">{output.packet.root.type === 'text' ? output.packet.root.text || '（空文字列）' : <pre>{JSON.stringify(output.packet.root, null, 2)}</pre>}</div><details className="packet-details"><summary>Packet JSON を表示</summary><pre>{JSON.stringify(output.packet, null, 2)}</pre></details></>}
        {output?.status === 'no-output' && <p className="output-state">この接続にはデータが出力されませんでした（NoOutput）。</p>}
        {output?.status === 'error' && <p className="output-state error">この接続はエラーのためデータがありません。{output.diagnostic.error.code}: {output.diagnostic.error.message}</p>}
        {!output && <p className="output-state">この接続はまだ実行されていません。現在のグラフを準備・切替してから実行してください。</p>}
      </div>}
      {ports.length === 0 && !connection && diagnostics.length === 0 && <p className="field-note">データの出力はありません。各ノードの実行状態を確認してください。</p>}
      <div className="execution-list">{Object.entries(result.nodes).map(([id, execution]) => <button onClick={() => onSelectNode(id)} key={id} className={`execution-chip ${execution.status}`}><i />{id}<ChevronRight size={11} /><span>{statusLabels[execution.status]}</span></button>)}</div>
    </>}
  </div>;
}
