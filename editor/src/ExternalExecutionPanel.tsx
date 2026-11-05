import ExecutionTimeline from './ExecutionTimeline';
import type { TraceConnection } from './useExecutionTrace';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createExternalApi, ExternalApiError, type ExternalClient, type ExternalReview, type ExternalStatus, type Approval, type SecretMetadata } from './externalApi';
import { graphKey, needsRuntimeInput, runtimeInputKey, type Graph, type GraphEdge, type Definition, type RuntimeInput, type NodeDisplayStatus } from './model';
import { sameRun, type ExecutionRunKey, type ExecutionSnapshot } from './executionModel';
import { decodeExecution } from './executionPayload';
import ExecutionResults from './ExecutionResults';
interface Props {
  timelineVisible?: boolean;
  graph: Graph; input: RuntimeInput; inputError?: string; composing: boolean; hasDrafts: boolean; connection?: GraphEdge;
  onClearConnection: () => void; onDefinitions: (nodes: Definition[]) => void; onCatalogError: () => void;
  onStatuses: (statuses: Record<string, NodeDisplayStatus> | null) => void; client?: ExternalClient;
}
interface Owner { id: string; generation: object }
interface Plan { graph: Graph; epoch: string }
interface Run { startKey: string; epoch: string; graph: string; input: string; key?: ExecutionRunKey; snapshot?: ExecutionSnapshot; unknown?: boolean }
function permissionLabel(request: Approval) {
  if (request.kind === 'networkUse') {
    const n = request.network!;
    return `${n.method} ${n.destination} への通信を許可（${request.nodes.join('、')}、最大 ${n.limits.maxCalls} 回・送信 ${n.limits.maxRequestBytes} バイト・受信 ${n.limits.maxResponseBytes} バイト、指定時間 ${n.limits.budgetMs / 1000} 秒。中止の完了には通信終了を待ちます）`;
  }
  if (request.kind === 'secretUse') {
    const n = request.network!;
    return `秘密 ${n.secret!.id}（版 ${n.secret!.version}）を ${n.method} ${n.destination} の認証に使用（${request.nodes.join('、')}）`;
  }
  if (request.kind === 'resourceRead') return request.resource!.binding + ' の ' + request.resource!.reference + ' を読み込む（' + request.nodes.join('、') + '）';
  return request.kind === 'execute' ? `${request.packageId}を実行（${request.nodes.join('、')}）` : `${request.source!.node}.${request.source!.port} → ${request.nodes.join('、')}への入力を許可`;
}
export default function ExternalExecutionPanel(props: Props) {
  const client = useRef(props.client ?? createExternalApi()).current, latest = useRef(props);latest.current = props;
  const owner = useRef<Owner | null>(null), alive = useRef(false), sequence = useRef(0n), runRef = useRef<Run | null>(null), statusRef = useRef<ExternalStatus | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [connected, setConnected] = useState(false), [busy, setBusy] = useState<string | null>(null), [controlBusy, setControlBusy] = useState(false);
  const [status, setStatus] = useState<ExternalStatus | null>(null), [review, setReview] = useState<ExternalReview | null>(null), [plan, setPlan] = useState<Plan | null>(null);
  const [run, setRun] = useState<Run | null>(null), [definitions, setDefinitions] = useState<Definition[]>([]), [selected, setSelected] = useState<string[]>([]), [saved, setSaved] = useState<string[]>([]);
  const [secretName, setSecretName] = useState(''), [secretValue, setSecretValue] = useState('');
  const [secretList, setSecretList] = useState<SecretMetadata[] | null>(null), [secretAvailable, setSecretAvailable] = useState(false);
  const manageSecret = async (mode: 'list' | 'put' | 'delete', name = secretName) => {
    if (!owner.current || busy || controlBusy || stopping) return;
    const value = secretValue;setSecretValue('');
    if (mode !== 'list') withdraw();
    await work(mode === 'list' ? '秘密一覧' : '秘密の更新', async (o, generation) => {
      if (mode === 'put') { const result = await client.secretPut(o.id, name, value);if (!current(o, generation)) return;acceptStatus(result.state); }
      if (mode === 'delete') { const state = await client.secretDelete(o.id, name);if (!current(o, generation)) return;acceptStatus(state); }
      const result = await client.secrets(o.id);if (!current(o, generation)) return;setSecretAvailable(result.available);setSecretList(result.entries);
    });
  };
  const [targetSelection, setTargetSelection] = useState<string[]>([]), [budget, setBudget] = useState('');
  const [issue, setIssue] = useState<string | null>(null), [stopping, setStopping] = useState(false), [invalidated, setInvalidated] = useState('');
  const attempt = useRef<{ valid: boolean; handedOff: boolean } | null>(null);
  const current = (o: Owner, generation: object) => alive.current && owner.current === o && o.generation === generation;
  const track = (value: Run | null) => { runRef.current = value;setRun(value); };
  const acceptStatus = (value: ExternalStatus) => {
    statusRef.current = value;setStatus(value);setUncertain(false);
    if (value.phase !== 'ready') { track(null);if (value.phase !== 'review') { setPlan(null);setReview(null);setSelected([]); } }
  };
  const withdraw = () => { const o = owner.current;if (o) o.generation = {};track(null);setPlan(null);setReview(null);setSelected([]);latest.current.onStatuses(null); };
  const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
  useEffect(() => { alive.current = true;return () => { alive.current = false;if (attempt.current) attempt.current.valid = false;const old = owner.current;owner.current = null;if (old) { old.generation = {};void client.close(old.id).catch(() => {}); } }; }, [client]);
  async function connect() {
    if (owner.current || busy || attempt.current) return;const connecting = { valid: true, handedOff: false };attempt.current = connecting;setBusy('接続');setIssue(null);
    let created: Owner | undefined;
    try {
      const value = await client.open();created = { id: value.connectionId, generation: {} };
      if (!alive.current || !connecting.valid) { await client.close(created.id);return; }
      owner.current = created;connecting.handedOff = true;const generation = created.generation;setConnected(true);acceptStatus(value.status);
      const catalog = await client.definitions(created.id);
      if (!current(created, generation)) return;setDefinitions(catalog.definitions);latest.current.onDefinitions(catalog.definitions);acceptStatus(catalog.state);const savedIds = await client.saved(created.id);if (current(created, generation)) setSaved(savedIds);
    } catch (error) { if (alive.current && connecting.valid && (!created || owner.current === created)) { setIssue(errorText(error));latest.current.onCatalogError(); } }
    finally { if (attempt.current === connecting) attempt.current = null;if (alive.current && (!connecting.handedOff || owner.current === created)) { setBusy(null);if (!connecting.handedOff) setStopping(false); } }
  }
  async function work(label: string, action: (o: Owner, generation: object) => Promise<void>) {
    const o = owner.current;if (!o || busy) return;const generation = o.generation;setBusy(label);setIssue(null);
    try { await action(o, generation); } catch (error) { if (current(o, generation)) {
      setIssue(errorText(error));
      if (label === '状態確認' || error instanceof ExternalApiError && error.unknownResult) { const tracked = runRef.current;if (tracked) track({ ...tracked, snapshot: undefined, unknown: true });setUncertain(true);latest.current.onStatuses(null); }
    } }
    finally { if (alive.current && owner.current === o) setBusy(null); }
  }
  const editedGraph = graphKey(props.graph), inputKey = needsRuntimeInput(props.graph) ? runtimeInputKey(props.input) : '';
  const matches = !!plan && graphKey(plan.graph) === editedGraph && !props.hasDrafts && plan.epoch === status?.sessionEpoch;
  useLayoutEffect(() => { const tracked = runRef.current;if (tracked) track({ ...tracked, snapshot: undefined, unknown: tracked.unknown || !tracked.key });latest.current.onStatuses(null); }, [editedGraph, props.hasDrafts, inputKey]);
  async function prepare() {
    await work('確認', async (o, generation) => {
      track(null);setReview(null);setPlan(null);setSelected([]);sequence.current = 0n;setInvalidated('');
      const graph = structuredClone(latest.current.graph), value = await client.prepare(o.id, graph);
      if (!current(o, generation) || graphKey(latest.current.graph) !== graphKey(graph) || latest.current.hasDrafts) return;
      setReview(value);setPlan({ graph, epoch: value.sessionEpoch });setStatus(previous => previous ? { ...previous, phase: 'review', sessionEpoch: value.sessionEpoch, reviewId: value.reviewId, state: null } : previous);
    });
  }
  async function approve(save: boolean) {
    if (!review || !matches || !selected.length) return;const reviewed = review;
    await work('承認', async (o, generation) => {
      const missing = await client.approve(o.id, reviewed, selected, save);
      if (!current(o, generation)) return;setReview({ ...reviewed, missing });setSelected([]);const savedIds = await client.saved(o.id);if (current(o, generation)) setSaved(savedIds);
    });
  }
  async function activate() {
    if (!review || !matches || review.missing.length) return;const reviewed = review;
    await work('切替', async (o, generation) => { const value = await client.activate(o.id, reviewed.reviewId);if (current(o, generation)) acceptStatus(value); });
  }
  const targets = useMemo(() => plan ? plan.graph.nodes.flatMap(node => definitions.find(d => d.kind === node.kind && d.version === node.version)?.outputs.map(p => ({ node: node.id, port: p.id })) ?? []) : [], [plan, definitions]);
  useEffect(() => { const ends = plan ? targets.filter(t => !plan.graph.edges.some(e => e.from.node === t.node && e.from.port === t.port)) : [];setTargetSelection((ends.length ? ends : targets).map(t => JSON.stringify(t))); }, [targets, plan]);
  const runnable = !uncertain && connected && status?.phase === 'ready' && !status.protected && matches && !busy && !controlBusy && !props.composing && !props.inputError && !run?.unknown && (!run || run.snapshot?.state === 'terminal');
  async function start() {
    if (!runnable || !plan || !status?.state?.activePlanVersion) return;
    await work('実行開始', async (o, generation) => {
      if (budget !== '' && (!/^[1-9][0-9]*$/.test(budget) || !Number.isSafeInteger(Number(budget)))) throw Error('実行期限は正の整数ms、または空欄にしてください。');
      if (sequence.current === 18446744073709551615n) throw Error('開始キーが上限に達しました。切断して接続し直してください。');
      const tracked: Run = { startKey: String(++sequence.current), epoch: plan.epoch, graph: editedGraph, input: inputKey };track(tracked);
      try {
        const value = await client.start(o.id, { startKey: tracked.startKey, expectedPlanVersion: status.state!.activePlanVersion!, targets: targetSelection.map(t => JSON.parse(t)), ...(budget === '' ? {} : { runBudgetMs: Number(budget) }),
          ...(needsRuntimeInput(plan.graph) ? { input: { ...latest.current.input } } : {}) });
        if (!current(o, generation) || (runRef.current?.epoch !== tracked.epoch || runRef.current?.startKey !== tracked.startKey)) return;
        acceptStatus(value.state);track({ ...tracked, key: value.key });
      } catch (error) {
        if (current(o, generation) && (runRef.current?.epoch === tracked.epoch && runRef.current?.startKey === tracked.startKey)) track(error instanceof ExternalApiError && !error.unknownResult ? null : { ...tracked, unknown: true });throw error;
      }
    });
  }
  async function inspect(o: Owner, generation: object, tracked: Run) {
    if (!tracked.key) return;const value = await client.getRun(o.id, tracked.key);
    if (!current(o, generation) || (runRef.current?.epoch !== tracked.epoch || runRef.current?.startKey !== tracked.startKey)) return;
    acceptStatus(value.state);
    if (value.state.phase === 'ready' && value.state.sessionEpoch === tracked.epoch && sameRun(value.snapshot.key, tracked.key)
      && (!tracked.snapshot || BigInt(value.snapshot.eventSeq) >= BigInt(tracked.snapshot.eventSeq))) track({ ...tracked, snapshot: value.snapshot });
  }
  async function query() {
    const tracked = runRef.current;if (!tracked?.unknown) return;
    await work('開始照会', async (o, generation) => {
      const value = await client.getStart(o.id, tracked.startKey);
      if (!current(o, generation) || (runRef.current?.epoch !== tracked.epoch || runRef.current?.startKey !== tracked.startKey) || value.key.sessionEpoch !== tracked.epoch) return;
      acceptStatus(value.state);track({ ...tracked, key: value.key, unknown: false });
    });
  }
  useEffect(() => {
    const o = owner.current;if (!o || busy || controlBusy || stopping) return;const generation = o.generation;
    const timer = setTimeout(() => { void work('状態確認', async () => {
      const tracked = runRef.current;
      if (tracked?.key && !tracked.unknown && !(tracked.snapshot?.state === 'terminal' && Object.values(tracked.snapshot.physical).every(n => n === 0))) await inspect(o, generation, tracked);
      else { const value = await client.status(o.id);if (current(o, generation)) acceptStatus(value); }
    }); }, run?.key && !run.unknown && run.snapshot?.state !== 'terminal' ? 100 : 1000);
    return () => clearTimeout(timer);
  }, [connected, status, run, busy, controlBusy, stopping]);
  // Input edits hide delivered data during render, then invalidate the Host without preparing/approving again.
  useEffect(() => {
    const o = owner.current, value = statusRef.current;
    if (!o || busy || controlBusy || !plan || value?.phase !== 'ready' || !value.state?.activePlanVersion || !needsRuntimeInput(plan.graph) || props.inputError) return;
    if ((value.state.latestInputRevision ?? 0) >= props.input.revision) return;
    const mark = JSON.stringify([value.sessionEpoch, props.input.revision]);if (invalidated === mark) return;
    void work('入力更新', async (_o, generation) => { const updated = await client.invalidate(o.id, value.state!.activePlanVersion!, latest.current.input.revision);if (current(o, generation)) { acceptStatus(updated);setInvalidated(mark); } });
  }, [props.input.revision, props.inputError, plan, status, busy, controlBusy, invalidated]);
  async function protection(enabled: boolean) {
    setSecretValue('');
    const o = owner.current;if (!o || controlBusy) return;withdraw();setControlBusy(true);setIssue(null);
    if (statusRef.current) acceptStatus({ ...statusRef.current, protected: true, phase: 'protected', state: null });
    try { await client.protect(o.id, enabled);if (alive.current && owner.current === o && statusRef.current) acceptStatus({ ...statusRef.current, protected: enabled, phase: enabled ? 'protected' : 'empty' }); }
    catch (error) { if (alive.current) setIssue(errorText(error)); }
    finally { if (alive.current) setControlBusy(false); }
  }
  async function revoke(id: string) {
    const o = owner.current;if (!o || controlBusy) return;withdraw();setControlBusy(true);setIssue(null);
    try { await client.revoke(o.id, id);if (alive.current && owner.current === o) { setSaved(previous => previous.filter(subject => subject !== id)); } }
    catch (error) { if (alive.current) setIssue(errorText(error)); }
    finally { if (alive.current) setControlBusy(false); }
  }
  async function disconnect() {
    setSecretValue('');setSecretList(null);setSecretAvailable(false);
    const o = owner.current;if (!o || stopping) return;if (attempt.current) attempt.current.valid = false;withdraw();owner.current = null;setStopping(true);setConnected(false);setStatus(null);latest.current.onCatalogError();
    try { await client.close(o.id); } catch (error) { if (alive.current) setIssue(errorText(error)); }
    finally { if (alive.current) { setStopping(false);setBusy(null); } }
  }
  useEffect(() => { const leave = () => { setSecretValue('');if (attempt.current) { attempt.current.valid = false;if (!owner.current) setStopping(true); }void disconnect(); };window.addEventListener('pagehide', leave);return () => window.removeEventListener('pagehide', leave); });
  const snapshot = run?.snapshot;
  const allowed = !uncertain && !!snapshot?.current && !!status?.state && status.phase === 'ready' && !status.protected && matches && run!.epoch === status.sessionEpoch
    && run!.graph === editedGraph && run!.input === inputKey && snapshot.key.deliveryGeneration === status.state.deliveryGeneration && snapshot.key.inputRevision === (needsRuntimeInput(props.graph) ? props.input.revision : undefined);
  const decoded = useMemo(() => allowed && snapshot?.result ? decodeExecution(snapshot.result, snapshot.key) : null, [allowed, snapshot]);
  const statuses = useMemo(() => decoded ? Object.fromEntries(decoded.nodes.map(n => [n.nodeId, n.status])) : null, [decoded]);
  useEffect(() => props.onStatuses(statuses), [statuses, props.onStatuses]);
  const traceConnection=useMemo<TraceConnection|null>(()=>{
    const o=owner.current;if(!o||!connected||status?.phase!=='ready'||status.protected||stopping)return null;
    const generation=o.generation,epoch=status.sessionEpoch;
    const guarded=async (operation:()=>ReturnType<ExternalClient['traceStart']>)=>{if(!current(o,generation)||statusRef.current?.sessionEpoch!==epoch||statusRef.current?.protected)throw Error('記録の接続が変わりました。');const value=await operation();if(!current(o,generation)||statusRef.current?.sessionEpoch!==epoch||statusRef.current?.protected)throw Error('記録の接続が変わりました。');return value;};
    return {scope:{},start:()=>guarded(()=>client.traceStart(o.id)),startDetailed:()=>guarded(()=>client.traceStartDetailed(o.id)),read:(after,limit)=>guarded(()=>client.traceRead(o.id,after,limit)),stop:()=>guarded(()=>client.traceStop(o.id)),clear:()=>guarded(()=>client.traceClear(o.id))};
  },[client,connected,status?.phase,status?.protected,status?.sessionEpoch,stopping,owner.current?.generation]);
  return <><div hidden={!!props.timelineVisible} className="v3-panel external-execution" aria-label="導入済みノードの実行">
    <div className="engine-actions"><strong>導入済みノード</strong><span role="status">{stopping ? '終了確認中' : busy ?? (status?.protected ? '保護中' : status?.phase === 'ready' ? '実行可能' : connected ? '接続済み' : '未接続')}</span>
      {!connected && <button className="button" disabled={!!busy || stopping} onClick={() => void connect()}>実行Hostに接続</button>}
      {connected && <><button className="button" disabled={!!busy || controlBusy} onClick={() => void work('一覧更新', async (o, generation) => { const value = await client.definitions(o.id);if (current(o, generation)) { setDefinitions(value.definitions);latest.current.onDefinitions(value.definitions);acceptStatus(value.state); } })}>ノード一覧を更新</button><button className="button" disabled={!!busy || controlBusy || props.hasDrafts || status?.protected} onClick={() => void prepare()}>グラフを確認</button>
        <button className="button primary" disabled={!runnable || !targetSelection.length} onClick={() => void start()}>承認済みグラフを実行</button>
        <button className="button" disabled={controlBusy} onClick={() => void protection(!status?.protected)}>{status?.protected ? '保護を解除' : '入力を保護'}</button>
        <button className="button" disabled={stopping} onClick={() => void disconnect()}>切断</button></>}
    </div>
    {issue && <p role="alert">{issue}</p>}
    {plan && <div className="v3-options"><label>実行期限（ms・空欄は無期限）<input aria-label="導入済みノードの実行期限" value={budget} inputMode="numeric" onChange={e => setBudget(e.target.value)} /></label><details><summary>実行する出力（{targetSelection.length}件）</summary>{targets.map(target => { const id = JSON.stringify(target);return <label key={id}><input type="checkbox" checked={targetSelection.includes(id)} onChange={e => setTargetSelection(s => e.target.checked ? [...s, id] : s.filter(v => v !== id))} />{target.node}.{target.port}</label>; })}</details></div>}
    {busy === '確認' && connected && <button className="button" onClick={() => { const o = owner.current;if (o) void client.cancel(o.id).catch(e => setIssue(errorText(e))); }}>確認を中止</button>}
    {review && matches && status?.phase !== 'ready' && <section aria-label="実行の承認"><h3>このグラフに許可する操作</h3><p>「今回だけ」はこの接続中のみ有効です。実装や入力経路が変わると、改めて確認します。</p>
      <ul>{review.requests.map(r => <li key={r.id}><label><input type="checkbox" disabled={!review.missing.some(m => m.id === r.id) || !!busy} checked={!review.missing.some(m => m.id === r.id) || selected.includes(r.id)} onChange={e => setSelected(s => e.target.checked ? [...s, r.id] : s.filter(id => id !== r.id))} />{permissionLabel(r)}</label><small> {r.packageId} · {r.id.slice(0, 12)}{!review.missing.some(m => m.id === r.id) && ' · 許可済み'}</small></li>)}</ul>
      <button className="button" disabled={!!busy || !selected.length} onClick={() => void approve(false)}>選択した操作を今回だけ許可</button>
      <button className="button" disabled={!!busy || !selected.length} onClick={() => void approve(true)}>選択した操作の許可を保存</button>
      <button className="button primary" disabled={!!busy || review.missing.length > 0} onClick={() => void activate()}>このグラフへ切替</button></section>}
    {!matches && plan && <p>グラフの変更を確認してから切り替えてください。</p>}
    {run?.unknown && <div role="alert">開始結果は不明です。再送せず開始キーを照会できます。<button className="button" disabled={!!busy} onClick={() => void query()}>開始結果を照会</button></div>}
    {run?.key && !run.unknown && (run.snapshot?.state !== 'terminal' || Object.values(run.snapshot.physical).some(count => count !== 0)) && <button className="button" disabled={!!busy} onClick={() => void work('中止', async (o, generation) => { const tracked = runRef.current;if (!tracked?.key) return;const value = await client.cancelRun(o.id, tracked.key);if (current(o, generation) && (runRef.current?.epoch === tracked.epoch && runRef.current?.startKey === tracked.startKey)) { acceptStatus(value.state);track({ ...tracked, snapshot: value.snapshot }); } })}>実行を中止</button>}
    {status && <p className="field-note">実行中の処理: {status.physical.physicalJobs ?? 0} / Worker: {status.workerProcesses}。中止・切断は処理の終了を待ちます。</p>}
    {run?.snapshot && <p>この実行: {run.snapshot.state} · 実行中 {run.snapshot.physical.running} · 取消待ち {run.snapshot.physical.cancelRequested} · 停止未確認 {run.snapshot.physical.stalled}</p>}
    {allowed && decoded && <div className="v3-diagnostics">{decoded.causes.map(c => <p key={c.id.nodeId}>{c.diagnostic.nodeId}: {c.diagnostic.error.code} — {c.diagnostic.error.message}</p>)}{decoded.recoveries.map((r, i) => <p key={i}>復旧: {r.nodeId} {r.input} → {r.output}</p>)}{snapshot?.result?.error && <p role="alert">{snapshot.result.error.code}: {snapshot.result.error.message}</p>}</div>}
    <ExecutionResults graph={props.graph} snapshot={allowed ? snapshot : undefined} allowed={allowed} connection={props.connection} onClearConnection={props.onClearConnection} />
    {connected && review && <details><summary>このグラフの許可</summary>{review.requests.filter(r => !review.missing.some(m => m.id === r.id)).map(r => <p key={r.id}>{permissionLabel(r)} <button className="button small" disabled={controlBusy} onClick={() => void revoke(r.id)}>この操作の許可を撤回</button></p>)}</details>}
    {connected && <details><summary>保存した許可（{saved.length}件）</summary>{saved.map(id => <p key={id}>{review?.requests.find(r => r.id === id) ? permissionLabel(review.requests.find(r => r.id === id)!) : id.slice(0, 16)} <button className="button small" disabled={controlBusy} onClick={() => void revoke(id)}>許可を撤回</button></p>)}</details>}
    {connected && <section aria-label="API秘密の管理"><button className="button" disabled={!!busy || controlBusy || stopping} onClick={() => void manageSecret('list')}>API秘密の一覧を更新</button>
      {secretList !== null && (!secretAvailable ? <p>このHostでは秘密の保存先が設定されていません。</p> : <>
        <p>秘密はこのWindowsユーザー用に暗号化して保存します。保存済みの値は表示できません。変更すると実行中のグラフを終了します。</p>
        <form autoComplete="off" onSubmit={e => { e.preventDefault();void manageSecret('put'); }}>
          <label>識別名<input aria-label="API秘密の識別名" value={secretName} autoComplete="off" onChange={e => setSecretName(e.target.value)} maxLength={128} /></label>
          <label>秘密の値<input aria-label="API秘密の値" type="password" autoComplete="new-password" spellCheck={false} value={secretValue} onChange={e => setSecretValue(e.target.value)} maxLength={4096} /></label>
          <button className="button" disabled={!!busy || controlBusy || !secretName || !secretValue}>秘密を保存（同じ識別名は置換）</button>
        </form>
        {secretList.map(entry => <p key={entry.id}>{entry.id} <button className="button small" disabled={!!busy || controlBusy} onClick={() => void manageSecret('delete', entry.id)}>この秘密を削除</button></p>)}
      </>)}
    </section>}
  </div><ExecutionTimeline connection={traceConnection} blocked={!!status?.protected} visible={!!props.timelineVisible} /></>;
}
