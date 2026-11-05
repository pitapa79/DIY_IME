import { useMemo, useState } from 'react';
import { traceConnections } from './graphReachability';
import { definitionFor, type Definition, type Graph } from './model';
interface Props { graph: Graph; definitions: Definition[]; initialSource?: string; remember: boolean; stored: boolean; onClose: () => void; external?: boolean }
export default function SecurityInfo({ graph, definitions, initialSource, remember, stored, onClose, external = false }: Props) {
  const [requested, setRequested] = useState(initialSource ?? '');
  const source = graph.nodes.some(node => node.id === requested) ? requested : graph.nodes[0]?.id;
  // A display budget, never a graph execution limit. Leave large graphs unresolved.
  const trace = useMemo(() => !source ? null : graph.nodes.length > 10000 || graph.edges.length > 20000
    ? { reached: [], issues: ['接続が多いため、この画面では経路を解析していません。'], connectionsResolved: false }
    : traceConnections(graph, definitions, source), [graph, definitions, source]);
  const selected = graph.nodes.find(node => node.id === source);
  return <div className="modal-backdrop" onClick={onClose}><section className="dialog security-dialog" role="dialog" aria-modal="true" aria-labelledby="security-info-title" onClick={event => event.stopPropagation()} onKeyDown={event => { if (event.key === 'Escape') onClose(); }}>
    <div className="dialog-heading"><h2 id="security-info-title">入力と実行の扱い</h2><button className="button small" autoFocus onClick={onClose}>閉じる</button></div>
    <dl><dt>実行方式</dt><dd>{external ? '導入済みWasmを隔離Workerで実行します。実行と入力経路の承認が必要です。' : '起動したRustコアに登録されたノードを実行します。'}</dd><dt>個別のOS権限管理</dt><dd>{external ? '通常のファイル・ネットワーク・子プロセスアクセスを遮断します。隔離を利用できない場合は実行を拒否します。' : '未接続。ノードごとのOSアクセスを制限する機能はありません。'}</dd><dt>保護入力</dt><dd>{external ? '実行画面の「入力を保護」で入力の受け渡しを止め、結果を非表示にできます。OSの保護入力の自動検出は未対応です。' : 'このEditorでは未対応です。'}</dd><dt>実行時入力の保存</dt><dd>{!stored ? '保存設定の反映に失敗しています。保存済み入力が残っている可能性があります。' : remember ? '選択により、このブラウザーへ保存します。' : '実行時入力はページ内だけに保持します。'}</dd></dl>
    <p>入力はノードから参照でき、結果や診断に含まれる場合があります。秘密情報を入力する用途にはまだ対応していません。</p>
    <p>グラフの固定文字列・設定は下書きとして自動保存されます。実行時入力の保存設定とは別です。</p>
    <h3>接続から到達し得るノード</h3>
    <p>現在編集中のグラフを解析します。すべての出力への可能性を含み、実行される経路を予測するものではありません。経路がなくても、入力の隔離を保証しません。</p>
    {source ? <><label className="field">解析元<select aria-label="接続経路の解析元" value={source} onChange={event => setRequested(event.target.value)}>{graph.nodes.map(node => <option key={node.id} value={node.id}>{node.id}</option>)}</select></label>
      <p>{selected && definitionFor(definitions, selected) ? '解析元のノード宣言を取得済みです。' : '解析元のノード宣言を取得できません。実行方式・権限は確認できません。'}</p>
      <p className="field-note">送信先・権限・危険度は未判定です。</p>
      {trace!.reached.length ? <ol className="security-paths">{trace!.reached.slice(0, 100).map(item => <li key={item.node}><strong>{item.node}</strong><code>{item.path.map(edge => `${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}`).join(' / ')}</code></li>)}</ol> : <p>解決できた接続からの到達先はありません。</p>}
      {trace!.reached.length > 100 && <p>到達先{trace!.reached.length}件のうち先頭100件を表示しています。</p>}
      {!trace!.connectionsResolved && <div className="field-error"><p>未解決の接続があります。表示した経路だけで判断できません。</p><ul>{trace!.issues.slice(0, 100).map(issue => <li key={issue}>{issue}</li>)}</ul>{trace!.issues.length > 100 && <p>未解決事項{trace!.issues.length}件のうち先頭100件を表示しています。</p>}</div>}
    </> : <p>グラフにノードがありません。</p>}
  </section></div>;
}
