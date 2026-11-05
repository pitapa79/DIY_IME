import { useMemo, useState } from 'react';
import type { Graph, GraphEdge } from './model';
import { decodeExecution, jsonPreview } from './executionPayload';
import type { ExecutionSnapshot } from './executionModel';
interface Props { graph?: Graph; snapshot?: ExecutionSnapshot; allowed: boolean; connection?: GraphEdge; onClearConnection: () => void }
export default function ExecutionResults({ graph, snapshot, allowed, connection, onClearConnection }: Props) {
  const decoded = useMemo(() => snapshot?.result ? decodeExecution(snapshot.result, snapshot.key) : null, [snapshot?.result, snapshot?.key]);
  const [selection, setSelection] = useState('');
  if (!snapshot?.result) return <p className="field-note">結果はまだ確定していません。</p>;
  if (!decoded) return <p role="alert">結果の形式を確認できません。データを表示しません。</p>;
  if (!allowed) return <p className="field-note">現在の入力・計画に採用できるデータはありません。旧結果は表示しません。</p>;
  const ports = decoded.ports, portId = (node: string, port: string) => JSON.stringify([node, port]);
  const targeted = ports.filter(item => snapshot.result!.targets.some(target => target.node === item.endpoint.node && target.port === item.endpoint.port));
  // Prefer a final node over an unused intermediate auxiliary port such as pending.
  const defaultOutput = targeted.find(item => graph && !graph.edges.some(edge => edge.from.node === item.endpoint.node)) ?? targeted[0] ?? ports[0];
  const chosen = connection ? portId(connection.from.node, connection.from.port) : selection;
  const selected = ports.find(item => portId(item.endpoint.node, item.endpoint.port) === chosen)
    ?? (connection ? undefined : defaultOutput);
  return <div className="v3-output">
    {snapshot.result.delivery === 'recovered' && <p className="tag amber">復旧した結果です。元の失敗と復旧箇所を下に残しています。</p>}
    {connection && <div className="connection-data"><strong>接続データ</strong><code>{connection.from.node}.{connection.from.port} → {connection.to.node}.{connection.to.port}</code><button className="button small" onClick={onClearConnection}>接続の選択を解除</button></div>}
    <label>表示する出力 <select aria-label="非同期の表示する出力" value={selected ? portId(selected.endpoint.node, selected.endpoint.port) : ''} onChange={event => { setSelection(event.target.value);onClearConnection(); }}>
      {!selected && <option value="">未実行の出力</option>}{ports.map(item => <option key={portId(item.endpoint.node, item.endpoint.port)} value={portId(item.endpoint.node, item.endpoint.port)}>{item.endpoint.node}.{item.endpoint.port}</option>)}
    </select></label>
    {selected?.value.status === 'data' && <div className="output-value" data-testid="v3-output-value">{selected.value.packet.root.type === 'text' ? selected.value.packet.root.text || '（空文字列）' : <pre>{jsonPreview(selected.value.packet.root)}</pre>}</div>}
    {selected?.value.status === 'no-output' && <p>この出力はNoOutputです。</p>}
    {selected?.value.status === 'error' && <p>この出力はエラーです。原因を確認してください。</p>}
    {selected?.value.status === 'discarded' && <p>この出力は採用されませんでした。</p>}
    {!selected && <p>この接続のデータは実行結果に含まれていません。</p>}
  </div>;
}
