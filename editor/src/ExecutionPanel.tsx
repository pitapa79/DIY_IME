import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createExecutionApi, ExecutionApiError } from './executionApi';
import { useExecutionSession, type ExecutionClient } from './useExecutionSession';
import { acceptExecution, advanceExecutionSnapshot, beginExecution, createExecutionView, executionDataVisible, executionIntent, executionPhysicalStopped,
  loseExecution, observeExecution, recoverExecution, updateExecutionContext, type ExecutionIntent, type ExecutionView } from './executionView';
import type { ExecutionRunKey, ExecutionSnapshot } from './executionModel';
import { decodeExecution, jsonPreview } from './executionPayload';
import { graphKey, needsRuntimeInput, runtimeInputKey, type Definition, type Endpoint, type Graph, type GraphEdge, type NodeDisplayStatus, type RuntimeInput } from './model';
import { useAutoRun } from './useAutoRun';
import ExecutionResults from './ExecutionResults';
interface Props {
  graph: Graph; input: RuntimeInput; inputError?: string; composing: boolean; hasDrafts: boolean; connection?: GraphEdge;
  onClearConnection: () => void; onDefinitions: (definitions: Definition[]) => void; onCatalogError: () => void;
  onStatuses: (statuses: Record<string, NodeDisplayStatus> | null) => void; onRestartInput: () => void;
  client?: ExecutionClient;
}
interface Plan { epoch: string; graph: Graph }
interface Prepared extends Plan { token: string }
interface Tracker { intent: ExecutionIntent; startKey: string; key?: ExecutionRunKey; snapshot?: ExecutionSnapshot; unknown?: boolean }
const labels = { idle: '待機中', starting: '開始の応答待ち', accepted: '開始受付済み', running: '実行中', terminal: '結果確定', unknown: '結果不明', forgotten: '結果の保持切れ' };
export default function ExecutionPanel(props: Props) {
  const clientRef = useRef(props.client ?? createExecutionApi()), client = clientRef.current;
  const core = useExecutionSession(true, client), latest = useRef(props);latest.current = props;
  const [active, setActive] = useState<Plan | null>(null), [prepared, setPrepared] = useState<Prepared | null>(null);
  const previous = useRef<Plan | null>(null), planRef = useRef<Plan | null>(null);planRef.current = active;
  const [view, setView] = useState<ExecutionView | null>(null), viewRef = useRef<ExecutionView | null>(null);
  const [tracked, setTracked] = useState<Tracker | null>(null), tracking = useRef<Tracker | null>(null);
  const [targets, setTargets] = useState<string[]>([]), [budget, setBudget] = useState(''), [issue, setIssue] = useState<string | null>(null), [notice, setNotice] = useState<string | null>(null);
  const [definitions, setDefinitions] = useState<Definition[]>([]), [catalog, setCatalog] = useState(false);
  const alive = useRef(false), sequence = useRef(0n), invalidated = useRef('');
  const publish = (next: ExecutionView | null) => { viewRef.current = next;if (alive.current) setView(next); };
  const track = (next: Tracker | null) => { tracking.current = next;if (alive.current) setTracked(next); };
  const sync = () => {
    const session = core.getSession();if (!session) return viewRef.current;
    const current = { sessionEpoch: session.id, state: session.state, input: planRef.current && needsRuntimeInput(planRef.current.graph) ? latest.current.input : null };
    const next = viewRef.current ? updateExecutionContext(viewRef.current, current) : createExecutionView(current);publish(next);return next;
  };
  const work = async (action: () => Promise<void>) => { const epoch = core.getSession()?.id;setIssue(null);setNotice(null);try { await action(); } catch (error) { if (alive.current && core.getSession()?.id === epoch) setIssue(error instanceof Error ? error.message : String(error)); } };
  useEffect(() => { alive.current = true;return () => { alive.current = false; }; }, []);
  useEffect(() => { if (core.connection === 'lost') { setCatalog(false);latest.current.onCatalogError(); } }, [core.connection]);
  useEffect(() => {
    const id = core.session?.id;if (!id) return;
    let disposed = false;setCatalog(false);setActive(null);planRef.current = null;setPrepared(null);previous.current = null;sequence.current = 0n;invalidated.current = '';track(null);publish(null);
    void core.call('definitions', sid => client.definitions(sid)).then(reply => {
      if (disposed) return;const nodes = reply.result.map(item => item.definition);setDefinitions(nodes);setCatalog(true);latest.current.onDefinitions(nodes);
    }).catch(() => { if (!disposed) latest.current.onCatalogError(); });
    return () => { disposed = true; };
  }, [core.session?.id, core.call, client]);
  // Derive a safe view in render too: a local edit must not briefly paint old Data.
  const currentContext = core.session ? { sessionEpoch: core.session.id, state: core.session.state, input: active && needsRuntimeInput(active.graph) ? props.input : null } : null;
  const display = view && currentContext ? updateExecutionContext(view, currentContext) : null;
  useLayoutEffect(() => {
    if (core.session) sync();
    else if (viewRef.current?.intent) publish(loseExecution(viewRef.current, viewRef.current.intent));
  }, [core.session, active, props.input]);
  useEffect(() => {
    const session = core.getSession(), plan = planRef.current;
    if (!session || session.state.latestInputRevision === null || !plan || !needsRuntimeInput(plan.graph) || props.inputError || core.busy || session.state.activePlanVersion === null
      || (session.state.latestInputRevision ?? 0) >= props.input.revision) return;
    const token = JSON.stringify([session.id, session.state.activePlanVersion, props.input.revision]);if (invalidated.current === token) return;invalidated.current = token;
    void core.call('invalidate', id => client.invalidate(id, session.state.activePlanVersion!, props.input.revision)).then(() => { if (core.getSession()?.id === session.id) sync(); }).catch(error => {
      if (core.getSession()?.id !== session.id) return;
      if (error instanceof ExecutionApiError && error.code === 'busy' && !error.state && !error.resultUnknown) invalidated.current = '';
      if (alive.current) setIssue(error instanceof Error ? error.message : String(error));
    });
  }, [core.session, core.busy, props.input.revision, props.inputError, active, core.call, client]);
  const outputTargets = useMemo(() => active ? active.graph.nodes.flatMap(node => definitions.find(item => item.kind === node.kind && item.version === node.version)?.outputs.map(port => ({ node: node.id, port: port.id })) ?? []) : [], [active, definitions]);
  useEffect(() => {
    if (!active) { setTargets([]);return; }
    const ends = outputTargets.filter(target => !active.graph.edges.some(edge => edge.from.node === target.node && edge.from.port === target.port));
    setTargets((ends.length ? ends : outputTargets).map(target => JSON.stringify(target)));
  }, [active, outputTargets]);
  const snapshot = display?.snapshot;
  const decoded = useMemo(() => snapshot?.result ? decodeExecution(snapshot.result, snapshot.key) : null, [snapshot?.result, snapshot?.key]);
  const graphMatches = !!active && active.epoch === core.session?.id && graphKey(active.graph) === graphKey(props.graph) && !props.hasDrafts;
  const visible = !!display && graphMatches && executionDataVisible(display);
  const statuses = useMemo(() => decoded && snapshot?.current && graphMatches ? Object.fromEntries(decoded.nodes.map(node => [node.nodeId, node.status])) : null, [decoded, snapshot?.current, graphMatches]);
  useEffect(() => props.onStatuses(statuses), [statuses, props.onStatuses]);
  const acceptSnapshot = (owner: Tracker, value: ExecutionSnapshot) => {
    if (tracking.current?.intent !== owner.intent || core.getSession()?.id !== owner.intent.sessionEpoch) return;
    const next = advanceExecutionSnapshot(tracking.current.snapshot, value);if (!next) return;
    track({ ...tracking.current, snapshot: next });const current = sync();if (current) publish(observeExecution(current, owner.intent, next));
  };
  const inspect = async (owner: Tracker) => {
    if (!owner.key) return;
    try { const reply = await core.call('get-run', id => client.getRun(id, owner.key!));acceptSnapshot(owner, reply.result); }
    catch (error) {
      if (tracking.current?.intent !== owner.intent) return;
      if (error instanceof ExecutionApiError && error.code === 'busy' && !error.state && !error.resultUnknown) return;
      const current = sync();if (current) publish(loseExecution(current, owner.intent, error instanceof ExecutionApiError && error.code === 'result-gone'));
      track({ ...owner, unknown: true });if (alive.current && core.getSession()?.id === owner.intent.sessionEpoch) setIssue(error instanceof Error ? error.message : String(error));
    }
  };
  useEffect(() => {
    if (!tracked?.key || tracked.unknown || !core.session || core.polling) return;
    const done = tracked.snapshot?.state === 'terminal' && Object.values(tracked.snapshot.physical).every(count => count === 0);if (done) return;
    const timer = setTimeout(() => { void inspect(tracked); }, Math.min(2_147_483_647, Math.max(50, tracked.snapshot?.pollAfterMs ?? 50)));
    return () => clearTimeout(timer);
  }, [tracked, core.session, core.polling]);
  const logicalBusy = !!tracked && !tracked.unknown && tracked.snapshot?.state !== 'terminal';
  const canRun = !!active && !!core.session && catalog && !core.busy && targets.length > 0 && !(needsRuntimeInput(active.graph) && props.inputError);
  const run = () => work(async () => {
    const session = core.getSession(), plan = planRef.current;if (!session || !plan || session.state.activePlanVersion === null || !targets.length) return;
    let runBudgetMs: number | undefined;
    if (budget !== '') { if (!/^[1-9][0-9]*$/.test(budget) || !Number.isSafeInteger(Number(budget))) throw new Error('実行期限は正の整数ms、または空欄にしてください。');runBudgetMs = Number(budget); }
    if (sequence.current === 18446744073709551615n) throw new Error('開始キーが上限に達しました。明示的に再接続してください。');
    auto.consume();const current = sync();if (!current) return;const intent = executionIntent(current.context), startKey = String(++sequence.current);
    const owner: Tracker = { intent, startKey };track(owner);publish(beginExecution(current, intent));
    try {
      const reply = await core.call('start', id => client.start(id, { startKey, expectedPlanVersion: session.state.activePlanVersion!, targets: targets.map(value => JSON.parse(value) as Endpoint),
        ...(intent.input ? { input: { ...intent.input } } : {}), ...(runBudgetMs === undefined ? {} : { runBudgetMs }) }));
      if (tracking.current?.intent !== intent || core.getSession()?.id !== intent.sessionEpoch) return;track({ ...tracking.current, key: reply.result.key });const next = sync();if (next) publish(acceptExecution(next, intent, reply.result));
    } catch (error) {
      if (tracking.current?.intent === intent) { const next = sync();const known = error instanceof ExecutionApiError && !error.resultUnknown;if (next) publish(known ? createExecutionView(next.context) : loseExecution(next, intent));track(known ? null : { ...owner, unknown: true }); }
      throw error;
    }
  });
  const autoScope = core.session && active && graphMatches && needsRuntimeInput(active.graph) ? JSON.stringify([core.session.id, core.session.state.activePlanVersion, graphKey(active.graph)]) : null;
  const auto = useAutoRun({ scope: autoScope, inputKey: runtimeInputKey(props.input), blocked: !!core.busy || logicalBusy || !!props.inputError || props.composing, run });
  const reconnect = () => work(async () => { auto.setEnabled(false);await core.connect();if (core.getSession()) latest.current.onRestartInput(); });
  return <section className="v3-panel" aria-label="非同期実行">
    <div className="v3-controls"><strong>非同期実行</strong><span className="tag">{core.connecting ? '接続中' : core.session ? active ? `稼働中 v${core.session.state.activePlanVersion}` : '計画未設定' : '未接続'}</span>
      <button className="button small" disabled={core.connecting} onClick={() => void reconnect()}>再接続</button>
      <button className="button small" disabled={!catalog || !core.session || !!core.busy || props.hasDrafts} onClick={() => void work(async () => { const epoch = core.getSession()?.id;const reply = await core.call('validate', id => client.validate(id, latest.current.graph));if (core.getSession()?.id === epoch) setNotice(`接続の検査が完了しました：${reply.result.order.join(' → ')}`); })}>検査</button>
      <button className="button small" disabled={!catalog || !core.session || !!core.busy || props.hasDrafts || !props.graph.nodes.length} onClick={() => void work(async () => { const epoch = core.getSession()?.id, graph = structuredClone(latest.current.graph), reply = await core.call('prepare', id => client.prepare(id, graph));if (!epoch || core.getSession()?.id !== epoch) return;setPrepared({ epoch, graph, token: reply.result.token });sync(); })}>計画を準備</button>
      <button className="button small" disabled={!prepared || prepared.epoch !== core.session?.id || graphKey(prepared.graph) !== graphKey(props.graph) || props.hasDrafts || !!core.busy} onClick={() => void work(async () => { if (!prepared) return;await core.call('activate', id => client.activate(id, prepared.token));if (core.getSession()?.id !== prepared.epoch) return;previous.current = planRef.current;planRef.current = prepared;setActive(prepared);setPrepared(null);sync(); })}>切替</button>
      <button className="button small" disabled={!core.session?.state.canRollback || !!core.busy} onClick={() => void work(async () => { const session = core.getSession();if (!session?.state.activePlanVersion) return;await core.call('rollback', id => client.rollback(id, session.state.activePlanVersion!));if (core.getSession()?.id !== session.id) return;planRef.current = previous.current;setActive(previous.current);previous.current = null;setPrepared(null);sync(); })}>復帰</button>
      <button className="button primary small" disabled={!canRun || logicalBusy} onClick={() => void run()}>実行</button>
      <button className="button small" disabled={!tracked?.key || tracked.snapshot?.state === 'terminal' || !core.session || core.cancelling} onClick={() => void work(async () => { const owner = tracking.current;if (!owner?.key) return;const reply = await core.call('cancel', id => client.cancel(id, owner.key!));acceptSnapshot(owner, reply.result); })}>取消</button>
    </div>
    {notice && <p className="field-note">{notice}</p>}
    {(issue || core.error) && <p role="alert" className="result-error">{issue ?? core.error}</p>}
    <div className="v3-options"><label>実行期限（ms・空欄は無期限）<input aria-label="非同期の実行期限" inputMode="numeric" value={budget} onChange={event => setBudget(event.target.value)} /></label>
      <label><input type="checkbox" aria-label="非同期の入力を自動実行" checked={auto.enabled} onChange={event => auto.setEnabled(event.target.checked)} />入力を自動実行</label>
      <details><summary>実行する出力（{targets.length}件）</summary>{outputTargets.map(target => { const id = JSON.stringify(target);return <label key={id}><input type="checkbox" checked={targets.includes(id)} onChange={event => setTargets(current => event.target.checked ? [...current, id] : current.filter(item => item !== id))} />{target.node}.{target.port}</label>; })}</details>
    </div>
    {active && !graphMatches && <p className="field-note">編集中のグラフは未反映です。現在のグラフへ旧結果を表示しません。</p>}
    {tracked && display?.intent !== tracked.intent && <p className="field-note">以前の実行の結果は非表示です。{tracked.snapshot?.state === 'terminal' && Object.values(tracked.snapshot.physical).every(count => count === 0) ? '以前の仕事の実際の処理終了を確認しました。' : '以前の仕事の終了は未確認です。'}</p>}
    <p role="status">{labels[display?.phase ?? (tracked?.unknown ? 'unknown' : 'idle')]}{snapshot ? ` · Core経過 ${snapshot.elapsedMs} ms` : ''}</p>
    {snapshot && <p>この実行の処理：実行中 {snapshot.physical.running}・取消待ち {snapshot.physical.cancelRequested}・停止未確認 {snapshot.physical.stalled}・待機 {snapshot.physical.queued}。{executionPhysicalStopped(display!) === true ? '実際の処理終了を確認しました。' : '実際の処理終了は未確認です。'}</p>}
    {snapshot?.result && <p>確定状態：{{ completed: '完了', cancelled: '結果の採用取消', expired: '期限切れ', failed: '失敗' }[snapshot.result.reason]} / {{ complete: '出力確定', recovered: '復旧した出力', 'no-output': '出力なし', failed: '出力エラー', cancelled: '出力取消', expired: '期限切れ' }[snapshot.result.delivery]}</p>}
    {tracked && !tracked.key && core.session && <button className="button small" disabled={core.polling} onClick={() => void work(async () => { const owner = tracking.current;if (!owner) return;const found = await core.call('get-start', id => client.getStart(id, owner.startKey));if (tracking.current?.intent !== owner.intent || core.getSession()?.id !== owner.intent.sessionEpoch) return;track({ ...owner, key: found.result.key, unknown: false });const next = sync();if (next) publish(next.phase === 'unknown' ? recoverExecution(next, owner.intent, found.result.key) : acceptExecution(next, owner.intent, { state: 'accepted', replayed: true, key: found.result.key })); })}>開始キーを照会（再送しません）</button>}
    <ExecutionResults graph={props.graph} snapshot={snapshot} allowed={visible} connection={props.connection} onClearConnection={props.onClearConnection} />
    {decoded && snapshot?.current && graphMatches && <div className="v3-diagnostics">{decoded.causes.map(cause => <p className="result-error" key={cause.id.nodeId}>{cause.diagnostic.nodeId}{cause.diagnostic.port ? `.${cause.diagnostic.port}` : ''}：{cause.diagnostic.error.code} — {cause.diagnostic.error.message}</p>)}
      {decoded.recoveries.map((record, index) => <p key={index}>復旧：{record.nodeId} {record.input} → {record.output}（原因：{record.causes.map(cause => cause.nodeId).join(', ')}）</p>)}
      {snapshot?.result?.error && <p role="alert">{snapshot.result.error.code}：{snapshot.result.error.message}</p>}
      <details><summary>実行状態と診断の詳細</summary><pre>{jsonPreview({ nodes: decoded.nodes, causes: decoded.causes, recoveries: decoded.recoveries })}</pre></details>
    </div>}
    {!core.session && !core.connecting && <p className="field-note">明示的に再接続して、計画を準備し直してください。以前の仕事の実際の処理終了は未確認です。</p>}
  </section>;
}
