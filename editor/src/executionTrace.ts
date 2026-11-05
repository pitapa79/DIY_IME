// Metadata-only diagnostics. Importing a capture never issues an execution request.
export const TRACE_EVENTS = ['run-accepted', 'dependency-wait', 'node-ready', 'node-started', 'node-resolved', 'node-skipped', 'node-failed', 'node-no-output', 'node-discarded', 'resource-wait', 'result-superseded', 'result-unavailable', 'call-cancelled', 'call-stalled', 'call-ended', 'core-result-ready', 'host-response-published', 'run-completed', 'no-output', 'run-cancelled', 'run-expired', 'run-failed', 'physical-ended', 'host-request-started', 'host-request-finished', 'worker-staging-started', 'worker-staging-finished', 'worker-process-started', 'worker-process-finished', 'worker-cleanup-started', 'worker-cleanup-finished', 'broker-exchange-started', 'broker-exchange-finished', 'worker-interval'] as const;
export type TraceKind = typeof TRACE_EVENTS[number];
export const WORKER_PHASES = ['main','dll-load','module-read','input-read','engine-setup','compile','instantiate','guest-setup','guest-call','reply-read','reply-write','cleanup'] as const;
export type WorkerPhase = typeof WORKER_PHASES[number];
export interface WorkerInterval { instance: number; phase: WorkerPhase; startedUs: string; finishedUs?: string }
export const workerPhaseLabels: Record<WorkerPhase,string> = { main:'計測対象全体', 'dll-load':'DLL読込', 'module-read':'モジュール読込', 'input-read':'入力パイプ読込', 'engine-setup':'Engine / Store準備', compile:'Wasmコンパイル', instantiate:'インスタンス生成', 'guest-setup':'呼出準備', 'guest-call':'ゲスト実行', 'reply-read':'応答の取出し', 'reply-write':'応答の送信', cleanup:'Runtime後片付け' };
export interface TraceEvent { sequence: string; atUs: string; run: number; input?: number; plan: number; node?: string; call?: number; nodeKind?: string; nodeVersion?: number; event: TraceKind; worker?: WorkerInterval }
export interface TracePage { coverage: 'new-runs-only'; formatVersion: 1; recordingId: string | null; recording: boolean; nowUs: string; firstSequence: string; nextSequence: string; dropped: number; events: TraceEvent[] }
export interface TraceCapture { format: 'diy-ime2-execution-trace'; version: 1; trace: TracePage }
export const TRACE_BYTES = 8 * 1024 * 1024, TRANSFER_BYTES = 16 * 1024 * 1024, TRACE_EVENT_LIMIT = 20_000;
const U64 = 18_446_744_073_709_551_615n;
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
function fields(v: unknown, required: string[], optional: string[] = []): v is Record<string, unknown> { return record(v) && required.every(k => Object.hasOwn(v, k)) && Object.keys(v).every(k => required.includes(k) || optional.includes(k)); }
export function traceDecimal(v: unknown): v is string { return typeof v === 'string' && /^(0|[1-9][0-9]{0,19})$/.test(v) && BigInt(v) <= U64; }
const positive = (v: unknown): v is number => Number.isSafeInteger(v) && Number(v) > 0 && Number(v) <= 4_294_967_295;
const bytes = (v: unknown) => new TextEncoder().encode(JSON.stringify(v)).length;
const invalid = (): never => { throw new Error('実行記録の形式または上限を確認してください。'); };
export function decodeTracePage(value: unknown, bulk = false): TracePage {
  if (!fields(value, ['coverage', 'formatVersion', 'recordingId', 'recording', 'nowUs', 'firstSequence', 'nextSequence', 'dropped', 'events'])
    || value.coverage !== 'new-runs-only' || value.formatVersion !== 1 || !(value.recordingId === null || typeof value.recordingId === 'string' && /^[0-9a-f]{32}$/.test(value.recordingId))
    || typeof value.recording !== 'boolean' || !traceDecimal(value.nowUs) || !traceDecimal(value.firstSequence) || !traceDecimal(value.nextSequence)
    || !Number.isSafeInteger(value.dropped) || Number(value.dropped) < 0 || !Array.isArray(value.events) || value.events.length > (bulk ? TRACE_EVENT_LIMIT : 256)) return invalid();
  let previous = 0n, at = 0n; const aliases = new Set<number>(), runIdentity=new Map<number,string>(), nodeIdentity=new Map<string,string>(), callIdentity=new Map<number,string>(), ended=new Set<string>();
  const workerIntervals = new Set<string>();
  for (const e of value.events) {
    if (!fields(e, ['sequence', 'atUs', 'run', 'plan', 'event'], ['input', 'node', 'call', 'nodeKind', 'nodeVersion', 'worker'])
      || !traceDecimal(e.sequence) || BigInt(e.sequence) <= previous || !traceDecimal(e.atUs) || BigInt(e.atUs) < at || BigInt(e.atUs) > BigInt(value.nowUs)
      || !positive(e.run) || !positive(e.plan) || typeof e.event !== 'string' || !TRACE_EVENTS.includes(e.event as TraceKind)
      || Object.hasOwn(e, 'input') && !positive(e.input) || Object.hasOwn(e, 'call') && !positive(e.call)
      || Object.hasOwn(e, 'node') && (typeof e.node !== 'string' || !/^[1-9][0-9]{0,9}:[1-9][0-9]{0,9}$/.test(e.node) || e.node.split(':')[0] !== String(e.plan))
      || Object.hasOwn(e, 'nodeKind') && (typeof e.nodeKind !== 'string' || [...e.nodeKind].length > 128)
      || Object.hasOwn(e, 'nodeVersion') && !positive(e.nodeVersion)
      || Object.hasOwn(e, 'call') && !Object.hasOwn(e, 'node')
      || Object.hasOwn(e, 'node') !== Object.hasOwn(e, 'nodeKind') || Object.hasOwn(e, 'node') !== Object.hasOwn(e, 'nodeVersion')) return invalid();
    const nodeEvent=['dependency-wait','resource-wait','node-ready','node-started','node-resolved','node-skipped','node-failed','node-no-output','node-discarded','call-cancelled','call-stalled','call-ended','host-request-started', 'host-request-finished', 'worker-staging-started', 'worker-staging-finished', 'worker-process-started', 'worker-process-finished', 'worker-cleanup-started', 'worker-cleanup-finished', 'broker-exchange-started', 'broker-exchange-finished', 'worker-interval'].includes(e.event);
    if(nodeEvent!==Object.hasOwn(e,'node') || ['node-started','resource-wait','call-cancelled','call-stalled','call-ended'].includes(e.event) && !Object.hasOwn(e,'call'))return invalid();
    if ((e.event === 'worker-interval') !== Object.hasOwn(e,'worker')) return invalid();
    if (e.event === 'worker-interval') {
      const w=e.worker;
      if(!fields(w,['instance','phase','startedUs'],['finishedUs']) || !positive(w.instance) || Number(w.instance)>20000 || !WORKER_PHASES.includes(w.phase as WorkerPhase)
        || !traceDecimal(w.startedUs) || Object.hasOwn(w,'finishedUs') && (!traceDecimal(w.finishedUs) || BigInt(w.finishedUs)<BigInt(w.startedUs)))return invalid();
      const key=`${e.run}/${e.node}/${w.instance}/${w.phase}`;if(workerIntervals.has(key))return invalid();workerIntervals.add(key);
    }
    const identity=String(e.plan)+'/'+String(e.input??0),nodeKey=String(e.run)+'/'+String(e.node);
    if(runIdentity.has(e.run)&&runIdentity.get(e.run)!==identity)return invalid();runIdentity.set(e.run,identity);
    if(e.node!==undefined){const node=String(e.node),kind=String(e.nodeKind)+'/'+String(e.nodeVersion);if(nodeIdentity.has(node)&&nodeIdentity.get(node)!==kind)return invalid();nodeIdentity.set(node,kind);if(!node.split(':').every(n=>positive(Number(n))))return invalid();}
    if(e.call!==undefined){if(callIdentity.has(Number(e.call))&&callIdentity.get(Number(e.call))!==nodeKey)return invalid();callIdentity.set(Number(e.call),nodeKey);}
    if(e.event==='node-started'&&ended.has(nodeKey))return invalid();if(e.event==='call-ended')ended.add(nodeKey);
    previous = BigInt(e.sequence); at = BigInt(e.atUs); aliases.add(e.run);
  }
  if (aliases.size > 200 || BigInt(value.firstSequence) > BigInt(value.nextSequence)
    || value.events.length && (BigInt(value.events[0].sequence) < BigInt(value.firstSequence) || previous > BigInt(value.nextSequence))
    || value.recordingId === null && (value.recording || value.events.length || value.nowUs !== '0' || value.nextSequence !== '0' || value.firstSequence !== '0')
    || bulk && bytes(value) > TRACE_BYTES) return invalid();
  // Copy only the validated schema; arbitrary imported properties cannot reach the view/export.
  return structuredClone(value) as unknown as TracePage;
}
export function emptyTrace(): TracePage { return { coverage: 'new-runs-only', formatVersion: 1, recordingId: null, recording: false, nowUs: '0', firstSequence: '0', nextSequence: '0', dropped: 0, events: [] }; }
export function mergeTrace(current: TracePage, page: TracePage): TracePage {
  if (page.recordingId === null) return emptyTrace();
  if (current.recordingId !== page.recordingId) current = emptyTrace();
  const last = current.events.at(-1)?.sequence ?? '0';
  const events = [...current.events.filter(e => BigInt(e.sequence) >= BigInt(page.firstSequence)), ...page.events.filter(e => BigInt(e.sequence) > BigInt(last))];
  const cutoff = BigInt(page.nowUs) > 60_000_000n ? BigInt(page.nowUs) - 60_000_000n : 0n;
  let start = events.findIndex(e => BigInt(e.atUs) >= cutoff); if (start < 0) start = events.length;
  const runIds = new Set<number>(); for (let i = events.length - 1; i >= start; i--) { runIds.add(events[i].run); if (runIds.size > 200) { start = i + 1; break; } }
  start = Math.max(start, events.length - TRACE_EVENT_LIMIT);
  let kept = events.slice(start), size = bytes({ ...page, events: kept });
  if (size > TRACE_BYTES) { let low = 0, high = kept.length; while (low < high) { const mid = (low + high) >>> 1; if (bytes({ ...page, events: kept.slice(mid) }) > TRACE_BYTES) low = mid + 1; else high = mid; } start += low; kept = kept.slice(low); size = bytes({ ...page, events: kept }); }
  if (size > TRACE_BYTES) return invalid();
  // Local eviction is visible as missing coverage, even if the server retained that page.
  return { ...page, firstSequence: kept[0]?.sequence ?? page.nextSequence, dropped: Math.max(current.dropped, page.dropped), events: kept };
}
export function exportTrace(trace: TracePage): string { const capture: TraceCapture = { format: 'diy-ime2-execution-trace', version: 1, trace: decodeTracePage(trace, true) }; const text = JSON.stringify(capture); if (new TextEncoder().encode(text).length > TRANSFER_BYTES) return invalid(); return text; }
export function importTrace(text: string): TracePage { if (new TextEncoder().encode(text).length > TRANSFER_BYTES) return invalid(); let v: unknown; try { v = JSON.parse(text); } catch { return invalid(); } if (!fields(v, ['format', 'version', 'trace']) || v.format !== 'diy-ime2-execution-trace' || v.version !== 1) return invalid(); return decodeTracePage(v.trace, true); }
export interface TraceRow { key: string; node: string; kind: string; version: number; call?: number; events: TraceEvent[]; dependency?: bigint; ready?: bigint; queued?: bigint; started?: bigint; resolved?: bigint; ended?: bigint; state: TraceKind }
export interface TraceRun { id: number; input?: number; origin: bigint; accepted: boolean; response?: bigint; ready?: bigint; ended?: bigint; rows: TraceRow[]; events: TraceEvent[] }
export function traceRuns(trace: TracePage): TraceRun[] {
  const runs = new Map<number, TraceRun>(), rows = new Map<string, TraceRow>();
  for (const e of trace.events) {
    let run = runs.get(e.run); if (!run) { run = { id: e.run, input: e.input, origin: BigInt(e.atUs), accepted: false, rows: [], events: [] }; runs.set(e.run, run); }
    const at = BigInt(e.atUs); run.events.push(e);
    if (e.event === 'run-accepted') { run.origin = at; run.accepted = true; }
    if (e.event === 'host-response-published') run.response ??= at;
    if (e.event === 'core-result-ready') run.ready ??= at;
    if (e.event === 'physical-ended') run.ended ??= at;
    if (!e.node) continue;
    // The Core currently issues at most one call per node per run. Call aliases remain explicit.
    let row = rows.get(`${e.run}/${e.node}`); if (!row) { row = { key: `${e.run}/${e.node}`, node: e.node, kind: e.nodeKind!, version: e.nodeVersion!, state: e.event, events: [] }; run.rows.push(row); rows.set(row.key,row); }
    row.events.push(e); row.state = e.event; if (e.call !== undefined) row.call = e.call;
    if (e.event === 'dependency-wait') row.dependency ??= at;
    if (e.event === 'node-ready') row.ready ??= at;
    if (e.event === 'resource-wait') row.queued ??= at;
    if (e.event === 'node-started') row.started ??= at;
    if (['node-resolved', 'node-skipped', 'node-failed', 'node-no-output', 'node-discarded'].includes(e.event)) row.resolved ??= at;
    if (e.event === 'call-ended') row.ended ??= at;
  }
  return [...runs.values()];
}
export const traceLabels: Record<TraceKind, string> = { 'run-accepted': '実行受理', 'dependency-wait': '依存待ち', 'node-ready': '実行可能', 'resource-wait': '実行枠待ち', 'node-started': '呼出開始', 'node-resolved': '結果確定', 'node-skipped': '未実行', 'node-failed': '失敗', 'node-no-output': '出力なし', 'node-discarded': '不要な処理', 'call-cancelled': '取消要求', 'call-stalled': '取消後も残存', 'call-ended': '呼出終了確認', 'core-result-ready': 'Core結果準備', 'host-response-published': '初回応答（Host公開）', 'run-completed': '実行完了', 'result-superseded': '古い結果', 'result-unavailable': '結果の保持不可', 'no-output': '出力なし', 'run-cancelled': '実行取消', 'run-expired': '期限切れ', 'run-failed': '実行失敗', 'physical-ended': '全処理の終了確認', 'host-request-started': 'Host要求準備開始', 'host-request-finished': 'Host要求準備退出', 'worker-staging-started': 'Worker準備開始', 'worker-staging-finished': 'Worker準備退出', 'worker-process-started': 'プロセス起動・待機開始', 'worker-process-finished': 'プロセス起動・待機退出', 'worker-cleanup-started': 'Worker後片付け開始', 'worker-cleanup-finished': 'Worker後片付け退出', 'broker-exchange-started': 'ネットワークbroker交換開始', 'broker-exchange-finished': 'ネットワークbroker交換退出', 'worker-interval': 'Worker内部区間の受信' };
export function traceMs(us: bigint): string { return (Number(us) / 1000).toLocaleString('ja-JP', { maximumFractionDigits: 3 }); }
