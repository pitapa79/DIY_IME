import { useEffect, useState } from 'react';
import { ArrowDown, Link2, X } from 'lucide-react';
import { connectionError, definitionFor, type Definition, type Graph, type GraphEdge } from './model';

export default function ConnectionDialog({ graph, definitions, onAdd, onClose }: { graph: Graph; definitions: Definition[]; onAdd: (edge: GraphEdge) => void; onClose: () => void }) {
  const sources = graph.nodes.filter(node => definitionFor(definitions, node)?.outputs.length);
  const targets = graph.nodes.filter(node => definitionFor(definitions, node)?.inputs.length);
  const [from, setFrom] = useState(sources[0]?.id ?? '');
  const [to, setTo] = useState(targets[0]?.id ?? '');
  const source = graph.nodes.find(node => node.id === from), target = graph.nodes.find(node => node.id === to);
  const outputs = source ? definitionFor(definitions, source)?.outputs ?? [] : [];
  const inputs = target ? definitionFor(definitions, target)?.inputs ?? [] : [];
  const [out, setOut] = useState(outputs[0]?.id ?? ''), [input, setInput] = useState(inputs[0]?.port.id ?? '');
  const edge = { from: { node: from, port: out }, to: { node: to, port: input } };
  const error = connectionError(graph, definitions, edge);
  useEffect(() => { const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); }; document.addEventListener('keydown', onKey); return () => document.removeEventListener('keydown', onKey); }, [onClose]);
  return <div className="modal-backdrop" onClick={onClose}><section className="dialog" role="dialog" aria-modal="true" aria-labelledby="connect-title" onClick={event => event.stopPropagation()}>
    <div className="dialog-heading"><span className="node-icon"><Link2 size={19} /></span><h2 id="connect-title">接続を追加</h2><button className="icon-button" aria-label="閉じる" onClick={onClose}><X size={18} /></button></div>
    <p>出力ポートから入力ポートへ接続します。</p>
    <div className="connection-fields"><label className="field">出力ノード<select autoFocus aria-label="出力ノード" value={from} onChange={event => { setFrom(event.target.value); const node = graph.nodes.find(node => node.id === event.target.value)!; setOut(definitionFor(definitions, node)?.outputs[0]?.id ?? ''); }}>{sources.map(node => <option value={node.id} key={node.id}>{node.id}</option>)}</select></label><label className="field">出力ポート<select aria-label="出力ポート" value={out} onChange={event => setOut(event.target.value)}>{outputs.map(port => <option value={port.id} key={port.id}>{port.id}</option>)}</select></label></div>
    <ArrowDown size={18} className="connection-arrow" />
    <div className="connection-fields"><label className="field">入力ノード<select aria-label="入力ノード" value={to} onChange={event => { setTo(event.target.value); const node = graph.nodes.find(node => node.id === event.target.value)!; setInput(definitionFor(definitions, node)?.inputs[0]?.port.id ?? ''); }}>{targets.map(node => <option value={node.id} key={node.id}>{node.id}</option>)}</select></label><label className="field">入力ポート<select aria-label="入力ポート" value={input} onChange={event => setInput(event.target.value)}>{inputs.map(({ port }) => <option value={port.id} key={port.id}>{port.id}</option>)}</select></label></div>
    {error && <p className="connection-hint" role="status">{error}</p>}
    <div className="dialog-footer"><button className="button" onClick={onClose}>キャンセル</button><button className="button primary" disabled={!!error} onClick={() => { onAdd(edge); onClose(); }}><Link2 size={15} />接続する</button></div>
  </section></div>;
}
