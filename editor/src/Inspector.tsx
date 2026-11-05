import { Braces, Check, SlidersHorizontal, Trash2, X } from 'lucide-react';
import { definitionFor, edgeId, label, type Definition, type Graph, type GraphNode, type GraphEdge } from './model';

interface Props {
  graph: Graph; definitions: Definition[]; node: GraphNode | undefined;
  draft: string; onDraft: (value: string) => void; onApply: () => void;
  onReset: () => void; onDelete: () => void; onRemoveEdge: (edge: GraphEdge) => void;
  onClose: () => void;
}
export default function Inspector({ graph, definitions, node, draft, onDraft, onApply, onReset, onDelete, onRemoveEdge, onClose }: Props) {
  const definition = node && definitionFor(definitions, node);
  const original = JSON.stringify(node?.config ?? definition?.defaultConfig ?? {}, null, 2);
  const dirty = draft !== original;
  let config: Record<string, unknown> | null = null, syntaxError = '';
  try { const value = JSON.parse(draft); if (value === null || typeof value !== 'object' || Array.isArray(value)) syntaxError = '設定は JSON オブジェクトで入力してください。'; else config = value; }
  catch (error) { syntaxError = error instanceof Error ? error.message : 'JSON を確認してください。'; }
  if (!syntaxError && ['demo.select-text', 'e2e.select-candidates', 'text.select-candidates'].includes(node?.kind ?? '') && (typeof config?.unscored !== 'number' || !Number.isFinite(config.unscored))) syntaxError = 'スコア省略時の評価値には有限の数値を入力してください。';
  if (!syntaxError && node?.kind === 'e2e.input' && (config?.mode !== 'fixed' || typeof config.text !== 'string' || [...config.text].length > 4096)) syntaxError = '固定文字列モードで、4096文字以内の文字列を入力してください。';
  const patch = (key: string, value: unknown) => onDraft(JSON.stringify({ ...config, [key]: value }, null, 2));
  const edges = node ? graph.edges.filter(edge => edge.from.node === node.id || edge.to.node === node.id) : [];
  const packet = config?.packet as { version: number; root: { type: string; text?: string } } | undefined;
  return <aside className="inspector">
    <div className="section-heading"><SlidersHorizontal size={16} /><h2>ノード設定</h2><span className="eyebrow">INSPECTOR</span><button className="icon-button compact-only" aria-label="ノード設定を閉じる" onClick={onClose}><X size={16} /></button></div>
    {!node ? <div className="inspector-empty"><SlidersHorizontal size={28} /><h3>ノードを選択</h3><p>キャンバスのノードを選ぶと、設定と接続を編集できます。</p></div> : <>
      <div className="inspector-title"><span className="tiny-label">選択中のノード</span><h3>{label(node.kind)}</h3><code>{node.id}</code></div>
      <div className="inspector-scroll"><section><div className="subheading"><h4>設定</h4>{dirty && <span className="tag amber">未適用</span>}</div>
        {!definition && <p className="field-error">この種別は Rust コアに登録されていません。</p>}
        {node.kind === 'input.text' && <p className="field-note">文章と入力モードはノード内で編集します。入力を変えても実行計画は変わりません。</p>}
        {node.kind === 'text.dictionary-exact' && config && <><label className="field">辞書ファイル<input aria-label="辞書ファイル" value={typeof config.path === 'string' ? config.path : ''} onChange={event => patch('path', event.target.value)} /><small>起動時に指定した辞書フォルダーからの相対名です。</small></label><p className="field-note">読み全体が一致した候補を出します。未登録の入力はそのまま残します。辞書の変更は「計画を準備」→「切替」で反映します。</p></>}
        {node.kind === 'text.stringify' && <p className="field-note">設定項目はありません。Text・選択済みSequenceを順番に連結します。Choiceが残っているとエラーになります。出力には入力のメタデータを付けません。</p>}
        {node.kind === 'text.romaji-to-hiragana' && config && <><label className="field">未知のローマ字<select aria-label="未知のローマ字" value={String(config.unknown ?? '')} onChange={event => patch('unknown', event.target.value)}><option value="preserve">原文を残す</option><option value="error">エラーにする</option></select><small>確定済みの文字は out、入力途中の末尾は pending に出力します。</small></label><p className="field-note">変換規則の追加・置換は設定 JSON の overrides で指定できます。例：{JSON.stringify({ from: 'ka', to: 'ヵ' })}</p></>}
        {node.kind === 'demo.branch' && config && <label className="field">出力する経路<select value={String(config.take ?? '')} onChange={event => patch('take', event.target.value)}><option value="yes">yes</option><option value="no">no</option></select><small>選ばれない経路には出力しません。</small></label>}
        {['demo.select-text', 'e2e.select-candidates', 'text.select-candidates'].includes(node.kind) && config && <label className="field">スコア省略時の評価値<input inputMode="decimal" value={typeof config.unscored === 'number' || typeof config.unscored === 'string' ? config.unscored : ''} onChange={event => { const raw = event.target.value; const value = Number(raw); patch('unscored', raw.trim() !== '' && Number.isFinite(value) ? value : raw); }} /><small>同じスコアなら先の候補を選びます。</small></label>}
        {node.kind === 'e2e.input' && config && <><label className="field">入力モード<select value="fixed" disabled><option value="fixed">固定文字列</option></select></label><label className="field">入力テキスト<textarea aria-label="入力テキスト" className="plain-text" value={typeof config.text === 'string' ? config.text : ''} onChange={event => patch('text', event.target.value)} rows={4} /><small>Issue #1 の固定規則で変換します。数字・記号はそのまま通します。</small></label></>}
        {node.kind === 'demo.packet' && config && packet?.root?.type === 'text' && <label className="field">入力テキスト<textarea className="plain-text" value={packet.root.text ?? ''} onChange={event => patch('packet', { ...packet, root: { ...packet.root, text: event.target.value } })} rows={4} /></label>}
        {node.kind === 'demo.packet' && packet?.root?.type !== 'text' && <p className="field-note">選択肢を含む候補木です。下の JSON から編集できます。</p>}
        {['demo.identity', 'demo.uppercase', 'e2e.romaji-to-hiragana', 'e2e.segment', 'e2e.kana-kanji', 'e2e.stringify', 'e2e.output'].includes(node.kind) && <p className="field-note">このノードには設定項目がありません。</p>}
        <label className="field json-field"><span><Braces size={13} />設定 JSON</span><textarea aria-label="設定 JSON" spellCheck={false} value={draft} onChange={event => onDraft(event.target.value)} rows={node.kind === 'demo.packet' ? 13 : 5} /></label>
        {syntaxError && <p className="field-error" role="alert">{syntaxError}</p>}
        <div className="form-actions"><button className="button primary small" disabled={!dirty || !!syntaxError} onClick={onApply}><Check size={14} />設定を適用</button><button className="button small" disabled={!dirty} onClick={onReset}>戻す</button></div>
      </section><section><div className="subheading"><h4>接続</h4><span className="muted">{edges.length}</span></div>
        {edges.length ? edges.map(edge => <div className="connection-row" key={edgeId(edge)}><div><code>{edge.from.node}.{edge.from.port}</code><span>→</span><code>{edge.to.node}.{edge.to.port}</code></div><button className="icon-button" aria-label={`接続を削除 ${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}`} title="接続を削除" onClick={() => onRemoveEdge(edge)}><X size={13} /></button></div>) : <p className="field-note">接続はありません。ポートをドラッグするか「接続を追加」を使ってください。</p>}
      </section><section className="node-info"><h4>ノード情報</h4><dl><dt>種別</dt><dd>{node.kind}</dd><dt>バージョン</dt><dd>{node.version}</dd><dt>データ</dt><dd>Text Packet v1</dd></dl></section></div>
      <div className="inspector-bottom"><button className="button danger" onClick={onDelete}><Trash2 size={15} />ノードを削除</button></div>
    </>}
  </aside>;
}
