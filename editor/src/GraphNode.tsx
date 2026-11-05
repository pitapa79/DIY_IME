import { Handle, Position, type Node, type NodeProps } from '@xyflow/react';
import { ArrowRight, Braces, CaseUpper, GitBranch, TextCursorInput, Languages, Scissors, ListFilter, AlignLeft, LogOut } from 'lucide-react';
import { label, type Definition, type GraphNode, type NodeDisplayStatus, type RuntimeInput } from './model';

export const icons = { 'text.dictionary-exact': Languages, 'text.select-candidates': ListFilter, 'text.stringify': AlignLeft, 'input.text': TextCursorInput, 'text.romaji-to-hiragana': Languages, 'e2e.input': TextCursorInput, 'e2e.romaji-to-hiragana': Languages, 'e2e.segment': Scissors, 'e2e.kana-kanji': Languages, 'e2e.select-candidates': ListFilter, 'e2e.stringify': AlignLeft, 'e2e.output': LogOut, 'demo.packet': Braces, 'demo.identity': ArrowRight, 'demo.uppercase': CaseUpper, 'demo.select-text': TextCursorInput, 'demo.branch': GitBranch };
export const statusLabels = { executed: '完了', skipped: 'スキップ', failed: 'エラー', blocked: '上流エラー', discarded: '採用対象外' };
interface InlineInput {
  text: string; disabled: boolean; error?: string;
  mode?: RuntimeInput['mode']; onModeChange?: (mode: RuntimeInput['mode']) => void;
  onFocus: () => void; onChange: (text: string) => void; onBlur: () => void;
  onCompositionStart?: () => void; onCompositionEnd?: () => void;
}
export type GraphFlowNode = Node<{ node: GraphNode; definition?: Definition; status?: NodeDisplayStatus; dirty: boolean; input?: InlineInput }, 'graph'>;

export default function GraphNodeView({ data, selected }: NodeProps<GraphFlowNode>) {
  const Icon = icons[data.node.kind as keyof typeof icons] ?? Braces;
  const config = (data.node.config ?? data.definition?.defaultConfig) as { text?: string; packet?: { root?: { type?: string; text?: string } }; take?: string } | undefined;
  const previews: Record<string, string> = { 'text.dictionary-exact': '全文一致 → 候補 / 未登録は原文', 'text.select-candidates': 'Choiceを選択 / Sequenceを保持', 'text.stringify': 'Text / 選択済み Sequence → Text', 'e2e.input': config?.text ?? '', 'text.romaji-to-hiragana': '確定済み out / 入力途中 pending', 'e2e.romaji-to-hiragana': 'ローマ字 → ひらがな', 'e2e.segment': 'Text → Choice〈Sequence〉', 'e2e.kana-kanji': '読み → 表記候補（構造を保持）', 'e2e.select-candidates': 'Choice ごとにスコアで選択', 'e2e.stringify': '選択済み Sequence → Text', 'e2e.output': 'Text → 表示 / stderr ログ' };
  const preview = previews[data.node.kind] ?? (data.node.kind === 'demo.packet' ? (typeof config?.packet?.root?.text === 'string' ? config.packet.root.text : '候補木を保持')
    : data.node.kind === 'demo.branch' ? `${config?.take ?? 'yes'} 経路へ出力` : data.node.kind === 'demo.select-text' ? 'スコアで選択 → 文章化' : 'Packet → Packet');
  return <div className={`graph-node ${selected ? 'is-selected' : ''} state-${data.status ?? 'idle'}`}>
    <div className="node-heading"><span className={`node-icon kind-${data.node.kind.split('.').at(-1)}`}><Icon size={18} /></span>
      <div><strong>{label(data.node.kind)}</strong><span className="node-id">{data.node.id}</span></div>{data.dirty && <i className="draft-dot" title="未適用の設定" />}</div>
    {data.input ? <div className="node-inline-input nodrag nopan nowheel" onClick={event => event.stopPropagation()} onDoubleClick={event => event.stopPropagation()}>
      <span>テキスト<small>{data.input.mode ? '実行時入力' : '固定文字列'}</small></span>
      <textarea aria-label={`${data.node.id} 入力テキスト`} className="nodrag nopan nowheel" spellCheck={false} autoComplete="off" rows={3}
        value={data.input.text} disabled={data.input.disabled} aria-invalid={!!data.input.error}
        onFocus={data.input.onFocus} onBlur={data.input.onBlur} onChange={event => data.input!.onChange(event.target.value)}
        onCompositionStart={data.input.onCompositionStart} onCompositionEnd={data.input.onCompositionEnd}
        onKeyDown={event => { if (!((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's')) event.stopPropagation(); }} />
      {data.input.error && <small className="inline-input-error" role="alert">{data.input.error}</small>}
      {data.input.mode && <select className="nodrag nopan nowheel" aria-label={`${data.node.id} 入力モード`} value={data.input.mode}
        onChange={event => data.input!.onModeChange!(event.target.value as RuntimeInput['mode'])}
        onKeyDown={event => event.stopPropagation()}><option value="compose">入力途中</option><option value="finalize">末尾を確定</option></select>}
    </div> : <div className="node-preview">{preview}</div>}
    <div className="node-ports"><div>{data.definition?.inputs.map(({ port, required }) => <div className="port input" key={port.id}>
      <Handle type="target" position={Position.Left} id={port.id} aria-label={`${data.node.id} 入力 ${port.id}`} />
      <span>{port.id}</span><small>{required ? '必須' : '任意'}</small></div>)}</div>
      <div>{data.definition?.outputs.map(port => <div className="port output" key={port.id}><span>{port.id}</span>
        <Handle type="source" position={Position.Right} id={port.id} aria-label={`${data.node.id} 出力 ${port.id}`} /></div>)}</div></div>
    <div className="node-footer"><span><i />{data.status ? statusLabels[data.status] : '未実行'}</span><code>v{data.node.version}</code></div>
  </div>;
}
