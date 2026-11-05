import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Background, BackgroundVariant, Controls, MarkerType, ReactFlow, useNodesInitialized, useReactFlow, type Connection, type NodeChange } from '@xyflow/react';
import { AlertCircle, ArrowRight, Braces, CheckCircle2, ChevronDown, Download, FilePlus2, FolderOpen, GitBranch, HelpCircle, LayoutGrid, Link2, Loader2, Play, Plus, Redo2, Search, ShieldCheck, SlidersHorizontal, Trash2, Undo2, Workflow, X } from 'lucide-react';
import { api } from './api';
import GraphNodeView, { icons, type GraphFlowNode } from './GraphNode';
import Inspector from './Inspector';
import SecurityInfo from './SecurityInfo';
import NodePackages from './NodePackages';
import ConnectionDialog from './ConnectionDialog';
import Results from './Results';
import ExecutionPanel from './ExecutionPanel';
import ExternalExecutionPanel from './ExternalExecutionPanel';
import { useCoreSession, type Operation } from './useCoreSession';
import { useRuntimeInput } from './useRuntimeInput';
import { useAutoRun } from './useAutoRun';
import { needsRuntimeInput, runtimeInputKey, type RuntimeInput, type NodeDisplayStatus } from './model';
import { commit, connectionError, definitionFor, edgeId, graphKey, initialDocument, label, layout, libraryCategories, metadata, newNodeId, parseGraph, redo, removeNode, undo, type Definition, type Document as EditorDocument, type Graph, type GraphEdge, type History, type Report } from './model';

const nodeTypes = { graph: GraphNodeView };
const EMPTY: Graph = { version: 1, nodes: [], edges: [] };
const STORAGE = 'diy-ime2.editor.v1';
const pretty = (value: unknown) => JSON.stringify(value, null, 2);

function inlineConfig(config: unknown, draft?: string) {
  try {
    const value = draft === undefined ? config : JSON.parse(draft);
    if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid config');
    return { config: value as Record<string, unknown>, disabled: false, error: undefined as string | undefined };
  } catch {
    const fallback = config && typeof config === 'object' && !Array.isArray(config) ? config as Record<string, unknown> : {};
    return { config: fallback, disabled: true, error: '設定 JSON を確認してください。' };
  }
}

function restore(): EditorDocument | null {
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE) ?? 'null');
    if (!stored) return null;
    const doc = initialDocument(parseGraph(JSON.stringify(stored.graph)), typeof stored.name === 'string' ? stored.name : 'untitled.json');
    for (const node of doc.graph.nodes) {
      const point = stored.positions?.[node.id];
      if (point && Number.isFinite(point.x) && Number.isFinite(point.y)) doc.positions[node.id] = point;
    }
    return doc;
  } catch { return null; }
}

export default function App({ externalOnly = import.meta.env.VITE_DISTRIBUTION === '1' }: { externalOnly?: boolean } = {}) {
  const [history, setHistory] = useState<History<EditorDocument>>({ past: [], present: initialDocument(EMPTY), future: [] });
  const doc = history.present, graph = doc.graph, key = graphKey(graph);
  const [definitions, setDefinitions] = useState<Definition[]>([]);
  const [catalogReady, setCatalogReady] = useState(false), [loadingCatalog, setLoadingCatalog] = useState(true);
  const core = useCoreSession(!externalOnly);
  const [runMode, setRunMode] = useState<'legacy' | 'async' | 'external'>(externalOnly ? 'external' : 'legacy');
  const executionMode = runMode !== 'legacy';
  const modeRef = useRef(externalOnly), catalogRequest = useRef<object>({});
  const [executionStatuses, setExecutionStatuses] = useState<Record<string, NodeDisplayStatus> | null>(null);
  const executionDefinitions = useCallback((nodes: Definition[]) => {
    if (!modeRef.current) return;setDefinitions(nodes);setCatalogReady(true);setLoadingCatalog(false);
  }, []);
  const executionCatalogError = useCallback(() => { if (modeRef.current) { setCatalogReady(false);setLoadingCatalog(false); } }, []);
  const runtime = useRuntimeInput();
  const [inputComposing, setInputComposing] = useState(false);
  const [recoveringInput, setRecoveringInput] = useState(false);
  const recovering = useRef(false);
  const ready = catalogReady && (executionMode || !!core.session && !core.connecting), booting = loadingCatalog || (!executionMode && core.connecting), busy = core.busy;
  const [selectedNode, setSelectedNode] = useState<string | null>(null), [selectedEdge, setSelectedEdge] = useState<string | null>(null);
  const [search, setSearch] = useState(''), [drafts, setDrafts] = useState<Record<string, string>>({});
  const [issue, setIssue] = useState<string | null>(null), [notice, setNotice] = useState<string | null>(null);
  const [result, setResult] = useState<{ key: string; graph: Graph; sessionId: string; report: Report; input?: RuntimeInput; duration: number } | null>(null);
  const [validation, setValidation] = useState<{ key: string; order: string[] } | null>(null);
  const [tab, setTab] = useState<'results' | 'json' | 'timeline'>('results'), [raw, setRaw] = useState(''), [rawBase, setRawBase] = useState('');
  useEffect(()=>{if(runMode!=='external' && tab==='timeline')setTab('results');},[runMode,tab]);
  const [connecting, setConnecting] = useState(false), [help, setHelp] = useState(false), [storageOk, setStorageOk] = useState(true);
  const [securityInfo, setSecurityInfo] = useState(false);
  const [nodePackages, setNodePackages] = useState(false);
  const [libraryOpen, setLibraryOpen] = useState(false), [inspectorOpen, setInspectorOpen] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null), librarySearch = useRef<HTMLInputElement>(null);
  const requestId = useRef(0), liveKey = useRef(key);
  const inputEdit = useRef<{ id: string; started: boolean } | null>(null);
  liveKey.current = key;
  const flow = useReactFlow<GraphFlowNode>();
  const nodesInitialized = useNodesInitialized();
  const nodeIds = graph.nodes.map(node => node.id).join('\0');
  const selected = graph.nodes.find(node => node.id === selectedNode);
  const selectedConnection = graph.edges.find(edge => edgeId(edge) === selectedEdge);
  const originalConfig = selected ? pretty(selected.config ?? definitionFor(definitions, selected)?.defaultConfig ?? {}) : '{}';
  const dirtyIds = graph.nodes.filter(node => Object.hasOwn(drafts, node.id) && drafts[node.id] !== pretty(node.config ?? definitionFor(definitions, node)?.defaultConfig ?? {})).map(node => node.id);
  const rawDirty = raw !== rawBase;
  const hasDrafts = dirtyIds.length > 0 || rawDirty;
  const activePlan = core.session?.state.active, prepared = core.session?.state.prepared;
  const matchesActive = !!activePlan && graphKey(activePlan.graph) === key;
  const canActivate = !!prepared && graphKey(prepared.graph) === key && !hasDrafts;
  const inputStale = !!result?.input && runtimeInputKey(result.input) !== runtimeInputKey(runtime.input);
  const currentResult = !executionMode && result?.key === key && !hasDrafts && !inputStale && result.sessionId === core.session?.id && result.report.planVersion === activePlan?.version;
  const validated = validation?.key === key && !hasDrafts;

  const fit = useCallback(() => { requestAnimationFrame(() => void flow.fitView({ padding: 0.19, maxZoom: 1, duration: 220 })); }, [flow]);
  const focusSelected = () => {
    if (selected) void flow.fitBounds(flow.getNodesBounds([selected.id]), { padding: 0.25, duration: 220 });
  };
  useEffect(() => { if (nodesInitialized) fit(); }, [nodesInitialized, nodeIds, fit]);
  useEffect(() => { window.addEventListener('resize', fit); return () => window.removeEventListener('resize', fit); }, [fit]);
  const loadDocument = useCallback((next: EditorDocument, reset = false) => {
    setInputComposing(false);
    setHistory(current => reset ? { past: [], present: next, future: [] } : commit(current, next));
    setSelectedNode(next.graph.nodes[0]?.id ?? null); setSelectedEdge(null); setDrafts({}); setIssue(null);
    setTab('results'); setRaw(''); setRawBase(''); setNotice(null); setResult(null); setValidation(null); setLibraryOpen(false); setInspectorOpen(false); fit();
  }, [fit]);

  useEffect(() => {
    let cancelled = false;
    if (externalOnly) {
      void api.sample('official-dictionary').then(sample => { if (!cancelled) loadDocument(restore() ?? initialDocument(sample, 'official-dictionary.json'), true); }).catch(error => { if (!cancelled) setIssue(String(error)); });
      return () => { cancelled = true; };
    }
    Promise.all([api.definitions(), api.sample('weather')]).then(([nodes, sample]) => {
      if (cancelled || modeRef.current) return;
      setDefinitions(nodes); setCatalogReady(true); loadDocument(restore() ?? initialDocument(sample, 'weather.json'), true);
    }).catch(error => { if (!cancelled) setIssue(error instanceof Error ? error.message : String(error)); })
      .finally(() => { if (!cancelled && !modeRef.current) setLoadingCatalog(false); });
    return () => { cancelled = true; };
  }, []);

  useLayoutEffect(() => {
    // 編集で常駐コアを停止しない。進行中の準備・切替の応答は状態だけを照合して受け取る。
    setValidation(null); setIssue(null);
  }, [key]);
  useEffect(() => {
    if (!catalogReady) return;
    try { localStorage.setItem(STORAGE, JSON.stringify(doc)); setStorageOk(true); } catch { setStorageOk(false); }
  }, [doc, catalogReady]);
  useEffect(() => { if (!notice) return; const timer = setTimeout(() => setNotice(null), 4500); return () => clearTimeout(timer); }, [notice]);

  const changeGraph = (update: (current: Graph) => Graph) => setHistory(current => commit(current, { ...current.present, graph: update(current.present.graph) }));
  const deleteSelection = () => {
    if (selectedNode) { changeGraph(current => removeNode(current, selectedNode)); setDrafts(current => { const next = { ...current }; delete next[selectedNode]; return next; }); setSelectedNode(null); setInspectorOpen(false); }
    else if (selectedEdge) { changeGraph(current => ({ ...current, edges: current.edges.filter(edge => edgeId(edge) !== selectedEdge) })); setSelectedEdge(null); }
  };
  const timeTravel = (direction: 'undo' | 'redo') => {
    setHistory(current => direction === 'undo' ? undo(current) : redo(current)); setDrafts({}); setSelectedNode(null); setSelectedEdge(null); setTab('results'); setRaw(''); setRawBase('');
  };
  const download = () => {
    if (hasDrafts) { setIssue('未適用の設定があります。設定またはグラフ JSON を適用してから保存してください。'); return; }
    const url = URL.createObjectURL(new Blob([pretty(graph) + '\n'], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = doc.name.endsWith('.json') ? doc.name : `${doc.name}.json`; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
    setNotice('グラフ JSON を保存しました。配置はこのブラウザーに保持されます。');
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const editing = event.target instanceof HTMLElement && (!!event.target.closest('input, textarea, select') || event.target.isContentEditable);
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') { event.preventDefault(); download(); }
      if (editing || connecting || help) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') { event.preventDefault(); timeTravel(event.shiftKey ? 'redo' : 'undo'); }
      if (event.key === 'Delete' || event.key === 'Backspace') { event.preventDefault(); deleteSelection(); }
    };
    window.addEventListener('keydown', onKey); return () => window.removeEventListener('keydown', onKey);
  });

  const addNode = (definition: Definition) => {
    const id = newNodeId(graph, definition.kind);
    const node = { id, kind: definition.kind, version: definition.version, config: structuredClone(definition.defaultConfig) };
    const anchor = selectedNode ? doc.positions[selectedNode] : null;
    const position = anchor ? { x: anchor.x + 330, y: anchor.y + 70 } : { x: 80 + graph.nodes.length % 3 * 80, y: 80 + graph.nodes.length * 45 };
    setHistory(current => commit(current, { ...current.present, graph: { ...current.present.graph, nodes: [...current.present.graph.nodes, node] }, positions: { ...current.present.positions, [id]: position } }));
    setSelectedNode(id); setSelectedEdge(null); setTab('results'); setLibraryOpen(false); setInspectorOpen(!['e2e.input', 'input.text'].includes(definition.kind)); fit();
  };
  const addConnection = (edge: GraphEdge) => {
    const error = connectionError(graph, definitions, edge);
    if (error) { setIssue(error); return; }
    changeGraph(current => ({ ...current, edges: [...current.edges, edge] })); setNotice('ポートを接続しました。');
  };
  const onConnect = (connection: Connection) => addConnection({ from: { node: connection.source, port: connection.sourceHandle ?? '' }, to: { node: connection.target, port: connection.targetHandle ?? '' } });
  const onNodesChange = (changes: NodeChange<GraphFlowNode>[]) => {
    const moved = changes.filter(change => change.type === 'position' && change.position);
    if (!moved.length) return;
    setHistory(current => { const positions = { ...current.present.positions }; for (const change of moved) if (change.type === 'position' && change.position) positions[change.id] = change.position; return { ...current, present: { ...current.present, positions } }; });
  };
  const updateInputText = (id: string, config: Record<string, unknown>, text: string) => {
    const recordUndo = inputEdit.current?.id !== id || !inputEdit.current.started;
    if (inputEdit.current?.id === id) inputEdit.current.started = true;
    setHistory(current => {
      const present = { ...current.present, graph: { ...current.present.graph, nodes: current.present.graph.nodes.map(node => node.id === id ? { ...node, config: { ...config, text } } : node) } };
      // 1回のフォーカス中の入力を、1操作として取り消せるようにする。
      return recordUndo ? commit(current, present) : { ...current, present, future: [] };
    });
    setDrafts(current => { const next = { ...current }; delete next[id]; return next; });
  };
  const nodes = useMemo<GraphFlowNode[]>(() => graph.nodes.map(node => {
    const definition = definitionFor(definitions, node);
    const settings = inlineConfig(node.config ?? definition?.defaultConfig ?? {}, Object.hasOwn(drafts, node.id) ? drafts[node.id] : undefined);
    const text = typeof settings.config.text === 'string' ? settings.config.text : '';
    const input = node.kind === 'input.text' ? {
      text: runtime.input.text, mode: runtime.input.mode, disabled: false, error: runtime.error,
      onFocus: () => { setSelectedNode(node.id); setSelectedEdge(null); },
      onChange: (text: string) => runtime.update({ text }),
      onModeChange: (mode: RuntimeInput['mode']) => runtime.update({ mode }),
      onCompositionStart: () => setInputComposing(true),
      onCompositionEnd: () => setInputComposing(false),
      onBlur: () => setInputComposing(false),
    } : node.kind === 'e2e.input' ? {
      text, disabled: settings.disabled, error: settings.error ?? ([...text].length > 4096 ? '4096文字以内で入力してください。' : undefined),
      onFocus: () => { inputEdit.current = { id: node.id, started: false }; setSelectedNode(node.id); setSelectedEdge(null); },
      onChange: (value: string) => updateInputText(node.id, settings.config, value),
      onBlur: () => { if (inputEdit.current?.id === node.id) inputEdit.current = null; },
    } : undefined;
    return { id: node.id, type: 'graph', position: doc.positions[node.id] ?? { x: 0, y: 0 }, selected: node.id === selectedNode,
      data: { node, definition, input, status: executionMode ? executionStatuses?.[node.id] : currentResult ? result?.report.nodes[node.id]?.status : undefined, dirty: dirtyIds.includes(node.id) } };
  }), [graph, doc.positions, selectedNode, definitions, currentResult, result, drafts, runtime.input, runtime.error, executionMode, executionStatuses]);
  const edges = graph.edges.map(edge => ({ id: edgeId(edge), source: edge.from.node, sourceHandle: edge.from.port, target: edge.to.node, targetHandle: edge.to.port,
    selected: edgeId(edge) === selectedEdge, label: edge.from.port === 'out' ? undefined : edge.from.port,
    ariaLabel: `ケーブル ${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}`, interactionWidth: 32,
    markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 }, style: { strokeWidth: 1.8 }, type: 'default' }));

  const applySettings = () => {
    if (!selected) return;
    try {
      const config = JSON.parse(Object.hasOwn(drafts, selected.id) ? drafts[selected.id] : originalConfig);
      if (!config || typeof config !== 'object' || Array.isArray(config)) throw new Error('設定は JSON オブジェクトで入力してください。');
      changeGraph(current => ({ ...current, nodes: current.nodes.map(node => node.id === selected.id ? { ...node, config } : node) }));
      setDrafts(current => { const next = { ...current }; delete next[selected.id]; return next; }); setNotice('設定を適用しました。');
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)); }
  };
  const perform = async (operation: Operation) => {
    if (executionMode || !ready || busy || recovering.current || (hasDrafts && ['validate', 'prepare', 'activate'].includes(operation))) return;
    if ((operation === 'run' || operation === 'rollback') && !activePlan) return;
    if (operation === 'activate' && !canActivate) return;
    const id = ++requestId.current, snapshot = key, start = performance.now(), sessionId = core.session!.id;
    setIssue(null); setTab('results');
    try {
      if (operation === 'run') {
        autoRun.consume();
        const running = activePlan!;
        const input = needsRuntimeInput(running.graph) ? structuredClone(runtime.live.current) : undefined;
        const reply = await core.call('run', sid => input ? api.runActive(sid, running.version, input) : api.runActive(sid, running.version));
        if (requestId.current === id && liveKey.current === snapshot) {
          if (input && runtimeInputKey(input) !== runtimeInputKey(runtime.live.current)) return;
          if (input && reply.result.inputRevision !== input.revision) throw new Error('実行結果の入力更新番号が一致しません。再実行してください。');
          setResult({ key: graphKey(running.graph), graph: running.graph, sessionId, report: reply.result, input, duration: performance.now() - start });
        }
      } else if (operation === 'validate') {
        const reply = await core.call('validate', sid => api.sessionValidate(sid, graph));
        if (requestId.current === id && liveKey.current === snapshot) { setValidation({ key: snapshot, order: reply.result.order }); setNotice('Rust コアで接続を検査しました。'); }
      } else if (operation === 'prepare') {
        const reply = await core.call('prepare', sid => api.prepare(sid, graph));
        if (liveKey.current === snapshot) { setValidation({ key: snapshot, order: reply.state.prepared!.order }); setNotice('計画を準備しました。「切替」で稼働計画へ反映します。'); }
        else setNotice('編集前の計画を準備しました。現在の編集を準備し直してください。');
      } else if (operation === 'activate') {
        const reply = await core.call('activate', sid => api.activate(sid, prepared!.token));
        setNotice(`稼働計画を v${reply.state.active!.version} に切り替えました。`);
      } else {
        const reply = await core.call('rollback', sid => api.rollback(sid, activePlan!.version));
        setNotice(`直前の構成へ復帰しました（稼働計画 v${reply.state.active!.version}）。編集中のグラフは保持します。`);
      }
    } catch (error) { if (requestId.current === id) setIssue(error instanceof Error ? error.message : String(error)); }
  };
  const autoScope = !executionMode && ready && matchesActive && !hasDrafts && activePlan && needsRuntimeInput(activePlan.graph)
    ? JSON.stringify([core.session!.id, activePlan.version, key]) : null;
  const autoRun = useAutoRun({
    scope: autoScope, inputKey: runtimeInputKey(runtime.input),
    blocked: !!busy || !!runtime.error || inputComposing,
    run: () => perform('run'),
  });
  const recoverInput = async () => {
    if (!runtime.exhausted || busy || core.connecting || recovering.current) return;
    recovering.current = true; setRecoveringInput(true);
    requestId.current++; autoRun.setEnabled(false); setInputComposing(false);
    setIssue(null); setResult(null); setValidation(null);
    try {
      await core.connect(() => {
        runtime.restartRevision();
        setNotice('入力を保持して再接続しました。計画を準備して切り替えてください。');
      });
    } finally { recovering.current = false; setRecoveringInput(false); }
  };
  const selectTab = (next: 'results' | 'json' | 'timeline') => { if (next === 'json' && tab !== 'json' && !rawDirty) { setRaw(pretty(graph)); setRawBase(pretty(graph)); } setTab(next); };
  const applyRaw = () => {
    try {
      const next = parseGraph(raw); const positions = { ...layout(next), ...doc.positions };
      loadDocument({ graph: next, positions, name: doc.name });
      setRaw(pretty(next)); setRawBase(pretty(next)); setTab('json'); setNotice('グラフ JSON を適用しました。');
    } catch (error) { setIssue(error instanceof Error ? error.message : String(error)); }
  };
  const importFile = async (file: File) => {
    try { if (file.size > 1024 * 1024) throw new Error('グラフは 1 MiB 以下にしてください。'); loadDocument(initialDocument(parseGraph(await file.text()), file.name)); setNotice(`${file.name} を読み込みました。`); }
    catch (error) { setIssue(error instanceof Error ? error.message : String(error)); }
  };

  const changeExecutionMode = (mode: 'legacy' | 'async' | 'external') => {
    if (externalOnly && mode !== 'external') return;
    const next = mode !== 'legacy';modeRef.current = next;const token = {};catalogRequest.current = token;
    autoRun.setEnabled(false);requestId.current++;setResult(null);setValidation(null);setIssue(null);setExecutionStatuses(null);
    setDefinitions([]);setCatalogReady(false);setLoadingCatalog(true);setRunMode(mode);
    if (!next) void api.definitions().then(nodes => {
      if (catalogRequest.current !== token || modeRef.current) return;setDefinitions(nodes);setCatalogReady(true);
    }).catch(error => { if (catalogRequest.current === token) setIssue(String(error)); })
      .finally(() => { if (catalogRequest.current === token) setLoadingCatalog(false); });
  };
  return <div className={`app-shell ${libraryOpen ? 'show-library' : ''} ${inspectorOpen ? 'show-inspector' : ''}`}>
    <header className="app-header"><div className="brand"><span className="brand-mark"><Workflow size={22} /></span><div><strong>DIY_IME<span>2</span></strong><small>GRAPH EDITOR</small></div></div>
      <div className="document-name"><span className="header-divider" /><span className="document-symbol"><GitBranch size={15} /></span><input aria-label="ファイル名" value={doc.name} onChange={event => setHistory(current => ({ ...current, present: { ...current.present, name: event.target.value } }))} /><span className="tag neutral">試作</span></div>
      <div className="header-actions"><span className={`core-status ${!executionMode && ready ? 'connected' : ''}`}><i />{executionMode ? runMode === 'external' ? '導入済みノード' : '非同期モード' : booting ? '接続中' : ready ? 'Rust 常駐' : '未接続'}</span>{!externalOnly && <><button className="button" aria-pressed={executionMode} disabled={!!busy || (!executionMode && loadingCatalog)} onClick={() => changeExecutionMode(runMode === 'legacy' ? 'async' : 'legacy')}>{executionMode ? '同期実行に戻る' : '非同期実行'}</button><button className="button" aria-pressed={runMode === 'external'} disabled={!!busy} onClick={() => changeExecutionMode(runMode === 'external' ? 'legacy' : 'external')}>{runMode === 'external' ? '導入済みノードを閉じる' : '導入済みノードで実行'}</button></>}
        {!executionMode && <><button className="button" disabled={!ready || hasDrafts || !!busy} onClick={() => void perform('validate')} title={hasDrafts ? '未適用の設定を適用してください' : 'Rust コアで接続を検査'}>{busy === 'validate' ? <Loader2 size={15} className="spin" /> : <ShieldCheck size={15} />}検査</button>
        <button className="button primary run-button" disabled={!ready || !activePlan || !!busy || (!!activePlan && needsRuntimeInput(activePlan.graph) && !!runtime.error)} onClick={() => void perform('run')} title={activePlan ? `稼働中の計画 v${activePlan.version} を実行` : '計画を準備して切り替えてください'}>{busy === 'run' ? <Loader2 size={15} className="spin" /> : <Play size={15} fill="currentColor" />}実行</button></>}</div>
    </header>
    <div className="workspace">
      <aside className="library"><button className="icon-button compact-only library-close" aria-label="ライブラリを閉じる" onClick={() => setLibraryOpen(false)}><X size={17} /></button><div className="library-heading"><span className="tiny-label">BUILD YOUR FLOW</span><h1>ノードライブラリ</h1><p>小さな処理を、つないで育てる。</p></div>
        <label className="search-box"><Search size={15} /><input aria-label="ノードを検索" ref={librarySearch} placeholder="ノードを検索" value={search} onChange={event => setSearch(event.target.value)} /></label>
        <div className="library-list">{(runMode === 'external' ? [...libraryCategories, '導入済み'] : libraryCategories).map(category => {
          const group = definitions.filter(definition => (runMode === 'external' && !libraryCategories.includes(metadata[definition.kind]?.category) ? '導入済み' : metadata[definition.kind]?.category) === category && `${label(definition.kind)} ${definition.kind}`.toLowerCase().includes(search.toLowerCase()));
          return group.length > 0 && <section key={category}><h2>{category}<span>{group.length.toString().padStart(2, '0')}</span></h2>{group.map(definition => { const Icon = icons[definition.kind as keyof typeof icons] ?? Braces; return <button className="library-node" key={`${definition.kind}@${definition.version}`} onClick={() => addNode(definition)} aria-label={`${label(definition.kind)}を追加`}>
            <span className={`node-icon kind-${definition.kind.split('.').at(-1)}`}><Icon size={17} /></span><span><strong>{label(definition.kind)}</strong><small>{metadata[definition.kind]?.description}</small></span><Plus size={14} className="library-plus" /></button>; })}</section>;
        })}{ready && !definitions.some(definition => (runMode === 'external' || libraryCategories.includes(metadata[definition.kind]?.category)) && `${label(definition.kind)} ${definition.kind}`.toLowerCase().includes(search.toLowerCase())) && <p className="field-note">一致するノードはありません。</p>}</div>
        <div className="library-note"><span className="note-dots"><i /><i /><i /></span><strong>まずは、つないでみる</strong><p>ノードを追加し、出力から入力へ線をつなぎます。</p><button onClick={() => setHelp(true)}>操作ガイド<ArrowRight size={13} /></button></div>
      </aside>
      <main className="workarea"><div className="document-toolbar"><button className="toolbar-button compact-only" onClick={() => { setLibraryOpen(!libraryOpen); setInspectorOpen(false); }} aria-label="ノードライブラリを開く"><Plus size={15} /><span>ノード</span></button><div className="file-actions"><button className="toolbar-button" title="新規グラフ" onClick={() => loadDocument(initialDocument(EMPTY))}><FilePlus2 size={15} /><span>新規</span></button><button className="toolbar-button" onClick={() => fileInput.current?.click()}><FolderOpen size={15} /><span>開く</span></button><button className="toolbar-button" onClick={download} disabled={hasDrafts}><Download size={15} /><span>保存</span></button><input ref={fileInput} type="file" accept=".json,application/json" hidden onChange={event => { const file = event.target.files?.[0]; if (file) void importFile(file); event.target.value = ''; }} /></div>
        <label className="sample-select"><span>サンプル</span><select aria-label="サンプルを読み込む" value="" onChange={event => { const name = event.target.value as 'official-dictionary' | 'dictionary-exact' | 'weather' | 'weather-negative' | 'runtime-input' | 'runtime-romaji' | 'stringify' | 'candidate-selection' | 'practical-pipeline'; void api.sample(name).then(sample => loadDocument(initialDocument(sample, `${name}.json`))).catch(error => setIssue(String(error))); }}><option value="" disabled>選択</option><option value="official-dictionary">同梱辞書で変換</option><option value="dictionary-exact">完全一致辞書（要フォルダー指定）</option><option value="stringify">実用文字列化</option><option value="practical-pipeline">実用ノード＋実行時入力</option><option value="candidate-selection">実用候補選択</option><option value="runtime-romaji">実用ローマ字変換</option><option value="runtime-input">実行時入力を確認</option><option value="weather">Issue #1：降水確率</option><option value="weather-negative">欠落入力：kaku なし</option></select><ChevronDown size={12} /></label>
        <div className="history-actions"><button className="icon-button" aria-label="元に戻す" title="元に戻す (Ctrl+Z)" disabled={!history.past.length} onClick={() => timeTravel('undo')}><Undo2 size={15} /></button><button className="icon-button" aria-label="やり直す" title="やり直す (Ctrl+Shift+Z)" disabled={!history.future.length} onClick={() => timeTravel('redo')}><Redo2 size={15} /></button></div></div>
        {(issue || (!executionMode && core.error)) && <div className="issue-banner" role="alert"><AlertCircle size={16} /><span>{issue ?? core.error}</span>{issue && <button aria-label="エラー表示を閉じる" onClick={() => setIssue(null)}><X size={15} /></button>}</div>}
        {!executionMode && <div className="engine-bar"><div className="engine-state"><span className={`tag ${activePlan ? 'green' : 'neutral'}`}>{activePlan ? `稼働中 v${activePlan.version}` : ready ? '計画未設定' : '未接続'}</span><small>{activePlan ? matchesActive && !hasDrafts ? '編集内容と一致' : '編集内容は未反映' : '準備 → 切替 → 実行'}</small>{prepared && <span className="tag amber">{canActivate ? '切替できます' : '別の編集を準備済み'}</span>}</div><label className="auto-run-control" title="実行時入力の計画を準備・切替した後、入力を変えると自動で実行します。編集中のグラフは自動で切り替えません。"><input type="checkbox" aria-label="入力を自動実行" checked={autoRun.enabled} onChange={event => autoRun.setEnabled(event.target.checked)} />入力を自動実行{autoRun.enabled && <small>{autoScope === null ? '切替後に開始' : inputComposing ? '文字の入力確定待ち' : busy ? '処理中' : 'ON'}</small>}</label><div className="engine-actions">
          {!core.session && !booting && <button className="button small" onClick={() => { requestId.current++; setIssue(null); setResult(null); setValidation(null); void core.connect(); }}>再接続</button>}
          {runtime.exhausted && (needsRuntimeInput(graph) || (activePlan && needsRuntimeInput(activePlan.graph))) && <button className="button small" disabled={booting || !!busy || recoveringInput} onClick={() => void recoverInput()} title="入力を保持して接続を作り直します。計画の準備と切替が必要です。">入力を保持して再接続</button>}
          <button className="button small" disabled={!ready || hasDrafts || !!busy || !graph.nodes.length} onClick={() => void perform('prepare')}>{busy === 'prepare' ? <Loader2 size={13} className="spin" /> : <ShieldCheck size={13} />}計画を準備</button>
          <button className="button small primary" disabled={!ready || !canActivate || !!busy} onClick={() => void perform('activate')}>{busy === 'activate' ? <Loader2 size={13} className="spin" /> : <ArrowRight size={13} />}切替</button>
          <button className="button small" disabled={!ready || !core.session?.state.canRollback || !!busy} onClick={() => void perform('rollback')} title="稼働計画を直前の構成へ戻す。編集中のグラフは保持します。"><Undo2 size={13} />復帰</button>
          {activePlan && !matchesActive && <button className="icon-button" aria-label="稼働中のグラフを開く" title="稼働中のグラフを編集画面へ読み込む" disabled={!!busy} onClick={() => loadDocument(initialDocument(structuredClone(activePlan.graph), doc.name))}><FolderOpen size={15} /></button>}
        </div></div>}
        {hasDrafts && <div className="draft-banner"><i /><span>未適用の変更があります。{rawDirty ? 'グラフ JSON' : 'ノード設定'}を適用してから計画を準備してください。実行は稼働計画を使います。</span>{dirtyIds.length > 0 && <button onClick={() => { setSelectedNode(dirtyIds[0]); setInspectorOpen(true); }}>設定を表示</button>}</div>}
        <section className="canvas" aria-label="グラフキャンバス"><div className="canvas-heading"><div><span className="tiny-label">WORKSPACE</span><span>{graph.nodes.length} ノード<span className="count-dot">·</span>{graph.edges.length} 接続</span></div><div><button className="canvas-button" onClick={() => setConnecting(true)} disabled={!graph.nodes.length}><Link2 size={13} />接続を追加</button><button className="canvas-button" onClick={() => { setHistory(current => ({ ...current, present: { ...current.present, positions: layout(current.present.graph) } })); fit(); }}><LayoutGrid size={13} />整列</button><button className="canvas-button" onClick={focusSelected} disabled={!selected} title="選択中のノードを編集しやすい倍率で表示"><Search size={13} />選択に拡大</button><button className="icon-button" aria-label="選択した接続を削除" title="選択した接続を削除" disabled={!selectedEdge} onClick={deleteSelection}><Trash2 size={14} /></button><button className="icon-button compact-only" aria-label="ノード設定を開く" onClick={() => { setInspectorOpen(!inspectorOpen); setLibraryOpen(false); }}><SlidersHorizontal size={15} /></button></div></div>
          {catalogReady && <ReactFlow<GraphFlowNode> nodes={nodes} edges={edges} nodeTypes={nodeTypes} onNodesChange={onNodesChange} onConnect={onConnect}
            isValidConnection={connection => connectionError(graph, definitions, { from: { node: connection.source!, port: connection.sourceHandle ?? '' }, to: { node: connection.target!, port: connection.targetHandle ?? '' } }) === null}
            onNodeClick={(_, node) => { setSelectedNode(node.id); setSelectedEdge(null); setInspectorOpen(true); setLibraryOpen(false); }} onEdgeClick={(_, edge) => { setSelectedEdge(edge.id); setSelectedNode(null); setTab('results'); setInspectorOpen(false); setLibraryOpen(false); }} onPaneClick={() => { setSelectedNode(null); setSelectedEdge(null); }}
            deleteKeyCode={null} minZoom={0.25} maxZoom={1.5} fitView fitViewOptions={{ padding: 0.19, maxZoom: 1 }} proOptions={{ hideAttribution: false }}>
            <Background variant={BackgroundVariant.Dots} gap={20} size={1} color="#cbd3d0" /><Controls showInteractive={false} />
          </ReactFlow>}
          {!catalogReady && <div className="canvas-empty"><p>{loadingCatalog ? 'ノードの宣言を確認しています。' : 'ノードの宣言を取得できません。接続状況を確認してください。'}</p></div>}
          {catalogReady && graph.nodes.length === 0 && <div className="canvas-empty"><span><Workflow size={30} /></span><h2>最初のノードを置いてみましょう</h2><p>ライブラリから、処理を選んで追加します。</p><button className="button" onClick={() => { setLibraryOpen(true); setInspectorOpen(false); requestAnimationFrame(() => librarySearch.current?.focus()); }}><Plus size={15} />ノードを探す</button></div>}
          <div className="canvas-caption"><span><i />ケーブルをクリックしてデータを確認</span><span>ドラッグで移動 · ホイールでズーム</span></div>
        </section>
        <section className={`result-panel ${executionMode ? 'v3-results-panel' : ''} ${selectedConnection ? 'is-inspecting' : ''}`}><div className="result-tabs"><button className={tab === 'results' ? 'active' : ''} onClick={() => selectTab('results')}><Play size={13} />実行結果{currentResult && result?.report.succeeded && <i className="success-dot" />}</button><button className={tab === 'json' ? 'active' : ''} onClick={() => selectTab('json')}><Braces size={14} />グラフ JSON</button>{runMode==='external' && <button className={tab==='timeline'?'active':''} onClick={()=>selectTab('timeline')}>実行タイムライン</button>}<span className="result-engine">{busy ? <><Loader2 size={12} className="spin" />処理中</> : validated ? <><CheckCircle2 size={12} />検査済み</> : 'Rust execution engine'}</span></div>
          {runMode === 'async' && <div hidden={tab !== 'results'} className="v3-panel-container"><ExecutionPanel graph={graph} input={runtime.input} inputError={runtime.error} composing={inputComposing} hasDrafts={hasDrafts} connection={selectedConnection} onClearConnection={() => setSelectedEdge(null)} onDefinitions={executionDefinitions} onCatalogError={executionCatalogError} onStatuses={setExecutionStatuses} onRestartInput={runtime.restartRevision} /></div>}
          {runMode === 'external' && <div hidden={tab !== 'results' && tab !== 'timeline'} className="v3-panel-container"><ExternalExecutionPanel timelineVisible={tab==='timeline'} graph={graph} input={runtime.input} inputError={runtime.error} composing={inputComposing} hasDrafts={hasDrafts} connection={selectedConnection} onClearConnection={() => setSelectedEdge(null)} onDefinitions={executionDefinitions} onCatalogError={executionCatalogError} onStatuses={setExecutionStatuses} /></div>}
          {tab === 'results' ? executionMode ? null : <Results result={result?.report ?? null} graph={result?.graph ?? graph} connection={selectedConnection} onClearConnection={() => setSelectedEdge(null)} stale={!currentResult} inputStale={inputStale} order={validated ? validation!.order : null} duration={result?.duration} onSelectNode={id => { setSelectedNode(id); setSelectedEdge(null); setInspectorOpen(true); }} /> : tab==='json' ? <div className="graph-json-editor"><textarea aria-label="グラフ JSON" spellCheck={false} value={raw} onChange={event => setRaw(event.target.value)} /><div><small>保存ファイルは Rust CLI と共通の形式です。</small><button className="button small" onClick={() => { setRaw(pretty(graph)); setRawBase(pretty(graph)); }} disabled={!rawDirty}>変更を破棄</button><button className="button primary small" onClick={applyRaw} disabled={!rawDirty}>JSON を適用</button></div></div> : null}
        </section>
      </main>
      <Inspector graph={graph} definitions={definitions} node={selected} draft={selected ? Object.hasOwn(drafts, selected.id) ? drafts[selected.id] : originalConfig : '{}'} onDraft={value => selected && setDrafts(current => ({ ...current, [selected.id]: value }))} onApply={applySettings}
        onReset={() => selected && setDrafts(current => { const next = { ...current }; delete next[selected.id]; return next; })} onDelete={deleteSelection} onRemoveEdge={edge => changeGraph(current => ({ ...current, edges: current.edges.filter(currentEdge => edgeId(currentEdge) !== edgeId(edge)) }))} onClose={() => setInspectorOpen(false)} />
    </div>
    <footer className="status-bar"><button onClick={() => setNodePackages(true)}>ノードの導入</button><button onClick={() => setSecurityInfo(true)}>入力と実行の扱い</button><label><input type="checkbox" aria-label="実行時入力をこのブラウザーに保存" checked={runtime.remember} onChange={event => runtime.setRememberInput(event.target.checked)} />実行時入力をこのブラウザーに保存</label><span><span className={`status-light ${ready ? 'ok' : ''}`} />{!runtime.stored ? '入力の保存設定を反映できません' : !storageOk ? '下書きを自動保存できません' : runtime.remember ? '下書きと実行時入力をブラウザーに保存' : 'グラフの下書きを保存・実行時入力はページ内のみ'}</span><span>Text Packet v1<span className="footer-separator">/</span>ローカル実行<button onClick={() => setHelp(true)} aria-label="操作ガイド"><HelpCircle size={13} /></button></span></footer>
    {notice && <div className="toast" role="status"><CheckCircle2 size={16} />{notice}</div>}
    {connecting && <ConnectionDialog graph={graph} definitions={definitions} onAdd={addConnection} onClose={() => setConnecting(false)} />}
    {nodePackages && <NodePackages onClose={() => setNodePackages(false)} />}
    {securityInfo && <SecurityInfo external={runMode === 'external'} graph={graph} definitions={definitions} initialSource={selectedNode ?? undefined} remember={runtime.remember} stored={runtime.stored} onClose={() => setSecurityInfo(false)} />}
    {help && <div className="modal-backdrop" onClick={() => setHelp(false)}><section className="dialog help-dialog" role="dialog" aria-modal="true" aria-labelledby="help-title" onClick={event => event.stopPropagation()}><div className="dialog-heading"><Workflow size={20} /><h2 id="help-title">はじめのグラフを動かす</h2><button className="icon-button" aria-label="ガイドを閉じる" onClick={() => setHelp(false)}><X size={18} /></button></div><ol><li><strong>ノードを追加</strong><p>左のライブラリから選びます。サンプルの読み込みもできます。</p></li><li><strong>設定して、つなぐ</strong><p>ノードを選び、右で設定を編集して適用。ポートをドラッグするか「接続を追加」でつなぎます。</p></li><li><strong>準備して、切り替えて実行</strong><p>「計画を準備」で検査し、「切替」で反映。「実行」は稼働中の計画を使います。無効な編集でも旧計画は残り、「復帰」で直前の構成へ戻せます。</p></li></ol><p className="field-note">ノード・接続の削除は Delete、元に戻すは Ctrl+Z、JSON 保存は Ctrl+S。配置はこのブラウザーに保持されます。試作のため、ブラウザーを閉じる前に必要なグラフを保存してください。</p><button className="button primary" onClick={() => setHelp(false)}>編集を始める<ArrowRight size={14} /></button></section></div>}
  </div>;
}
