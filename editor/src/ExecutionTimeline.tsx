import { useMemo, useRef, useState } from 'react';
import { label } from './model';
import { exportTrace, traceLabels, traceMs, traceRuns, type TracePage, type TraceRow, type TraceRun } from './executionTrace';
import { useExecutionTrace, type TraceConnection } from './useExecutionTrace';
import './executionTrace.css';
import WorkerTraceDetails from './WorkerTraceDetails';
interface Props { connection: TraceConnection | null; blocked: boolean; visible: boolean }
function Chart({ trace, live, title }: { trace: TracePage; live: boolean; title: string }) {
  const runs = useMemo(() => traceRuns(trace), [trace]);
  const [chosen, setChosen] = useState<number | null>(null), [selected, setSelected] = useState<string | null>(null), [page, setPage] = useState(0);
  const run = runs.find(r => r.id === chosen) ?? runs.at(-1);
  const detail = run?.rows.find(r => r.key === selected);
  if (!run) return <div className="trace-empty">{live && trace.recording ? '記録開始後に新しく実行した処理を表示します。' : live ? '記録は既定でOFFです。記録開始後にグラフを実行してください。' : 'この固定記録には工程イベントがありません。'}</div>;
  const end = run.ended !== undefined ? run.events.reduce((m, e) => BigInt(e.atUs) > m ? BigInt(e.atUs) : m, run.ended) : BigInt(trace.nowUs);
  const rawMs = Math.max(0.001, Number(end - run.origin) / 1000), unit = 10 ** Math.floor(Math.log10(rawMs / 3));
  const step = ([1, 2, 4, 5, 10].find(n => n * unit >= rawMs / 3) ?? 10) * unit, maxMs = step * 3;
  const position = (at: bigint) => Math.max(0, Math.min(100, Number(at - run.origin) / 1000 / maxMs * 100));
  const count = Math.ceil(run.rows.length / 100), actualPage = Math.min(page, Math.max(0, count - 1));
  return <section className="trace-capture" aria-label={title}>
    <div className="trace-chart-heading"><strong>{title} · 実行タイムライン</strong><label>実行を選択 <select aria-label={`${title}の実行`} value={run.id} onChange={e => { setChosen(Number(e.target.value)); setSelected(null); setPage(0); }}>{runs.map(r => <option key={r.id} value={r.id}>{r.input === undefined ? '入力番号なし' : `入力 #${r.input}`} / 実行 #{r.id}</option>)}</select></label></div>
    {(trace.dropped > 0 || !run.accepted) && <p className="trace-gap" role="status">記録に欠落があります。開始・終了が分からない区間を完了とは表示しません。</p>}
    <div className={`trace-layout ${detail ? 'with-detail' : ''}`}>
      <div className="execution-trace-chart">
        <div className="trace-axis"><span>工程</span><div>{[0, 1, 2, 3].map(i => <span key={i} style={{ left: `${i / 3 * 100}%` }}>{traceMs(BigInt(Math.round(step * i * 1000)))}</span>)}<b>ms</b></div></div>
        <div className="trace-lanes">{run.rows.slice(actualPage * 100, actualPage * 100 + 100).map(row => <Lane key={row.key} row={row} run={run} now={run.ended ?? end} position={position} selected={detail?.key === row.key} gaps={trace.dropped > 0 || !run.accepted} live={live && trace.recording} onSelect={() => setSelected(row.key)} />)}</div>
        {run.response !== undefined && <div className="trace-marker-region" aria-label={`初回応答（Host公開） ${traceMs(run.response - run.origin)} ms`}><i className="trace-response-marker" style={{ left: `${position(run.response)}%` }}><span>初回応答 {traceMs(run.response - run.origin)} ms</span></i></div>}
        <p className="trace-measurement">{!run.accepted ? '起点未取得 · 最初に取得したイベントを表示起点に使用' : '実行受理からの経過'}{run.response === undefined ? ' · 初回応答は未記録' : ' · 初回応答はHostの送信完了時点'}{run.ready !== undefined && ` · Core結果準備 ${traceMs(run.ready - run.origin)} ms`}</p>
      </div>
      {detail && <aside className="trace-detail" aria-label="選択した工程の詳細"><div><strong>工程の詳細</strong><button type="button" aria-label="詳細を閉じる" onClick={() => setSelected(null)}>×</button></div><h4>{label(detail.kind)}</h4><dl><dt>工程番号</dt><dd>{detail.node}</dd><dt>呼出番号</dt><dd>{detail.call === undefined ? '呼出記録なし' : `#${detail.call}`}</dd><dt>種別・版</dt><dd>{detail.kind} / v{detail.version}</dd><dt>呼出開始</dt><dd>{detail.started === undefined ? '未取得' : `${traceMs(detail.started - run.origin)} ms`}</dd><dt>結果確定</dt><dd>{detail.resolved === undefined ? '未確認' : `${traceMs(detail.resolved - run.origin)} ms`}</dd><dt>呼出終了</dt><dd>{detail.ended === undefined ? '未確認' : `${traceMs(detail.ended - run.origin)} ms`}</dd><dt>呼出の経過</dt><dd>{detail.started === undefined ? '開始記録なし' : `${traceMs((detail.ended ?? run.ended ?? end) - detail.started)} ms${detail.ended === undefined ? run.ended===undefined ? '（終了未確認）' : '（個別時刻未取得・全体は終了）' : ''}`}</dd><dt>計測元</dt><dd>Core / Hostの単調時計。Worker区間は別時計</dd></dl><WorkerTraceDetails key={detail.key} row={detail}/><ol>{detail.events.map(e => <li key={e.sequence}><time>{traceMs(BigInt(e.atUs) - run.origin)} ms</time> {traceLabels[e.event]}</li>)}</ol><p>Host区間の「退出」は成功やOSプロセス終了の証明ではありません。プロセス区間は起動・待機・回収を含み、純Wasm時間とは異なります。</p></aside>}
    </div>
    {count > 1 && <nav className="trace-pagination" aria-label="工程一覧のページ"><button type="button" disabled={actualPage === 0} onClick={() => setPage(actualPage - 1)}>前の100工程</button><span>{actualPage * 100 + 1}–{Math.min((actualPage + 1) * 100, run.rows.length)} / {run.rows.length}</span><button type="button" disabled={actualPage + 1 >= count} onClick={() => setPage(actualPage + 1)}>次の100工程</button></nav>}
  </section>;
}
function Lane({ row, run, now, position, selected, gaps, live, onSelect }: { row: TraceRow; run: TraceRun; now: bigint; position: (n: bigint) => number; selected: boolean; gaps: boolean; live: boolean; onSelect(): void }) {
  const phase = row.events.slice().reverse().find(e => ['call-stalled', 'call-cancelled', 'node-failed', 'node-skipped', 'node-no-output', 'node-discarded'].includes(e.event));
  const unknownStart=row.started===undefined && phase?.event!=='node-skipped' && (gaps || row.ended!==undefined || row.resolved!==undefined || run.ended!==undefined);
  const status=row.ended!==undefined ? (phase ? traceLabels[phase.event]+'・' : '')+'呼出終了確認済み' : phase?.event==='node-skipped' ? '未実行' : run.ended!==undefined ? '個別終了時刻未取得（全体は終了）' : row.started!==undefined ? (phase ? traceLabels[phase.event]+'・' : '')+(live?'終了確認待ち':'この記録では終了未確認') : row.resolved!==undefined ? (phase?traceLabels[phase.event]:'結果確定')+(unknownStart?'・呼出開始未取得':'') : unknownStart?'待機・開始の区切り未取得':live?'待機中':'この記録では終了未確認';
  const start = row.started ?? row.ready ?? row.dependency;
  const width = (from: bigint, to: bigint) => Math.max(0.12, position(to) - position(from));
  return <div className={`trace-lane ${selected ? 'selected' : ''}`}><button className="trace-lane-name" type="button" onClick={onSelect} title={row.kind}>{label(row.kind)}<small>工程 {row.node}{row.call !== undefined && ` · 呼出 #${row.call}`}</small></button><button type="button" className="trace-track" aria-label={`${label(row.kind)}の詳細：${phase ? traceLabels[phase.event] : traceLabels[row.state]}`} aria-pressed={selected} onClick={onSelect}>
    {row.dependency !== undefined && <i className={`trace-segment dependency ${unknownStart ? 'uncertain' : ''}`} style={{ left: `${position(row.dependency)}%`, width: `${width(row.dependency, row.ready ?? row.resolved ?? row.ended ?? now)}%` }} />}
    {row.ready !== undefined && <i className={`trace-segment waiting ${unknownStart ? 'uncertain' : ''}`} style={{ left: `${position(row.ready)}%`, width: `${width(row.ready, row.started ?? row.resolved ?? row.ended ?? now)}%` }} />}
    {row.started !== undefined && <i className={`trace-segment executing ${phase ? 'attention' : ''} ${row.ended !== undefined ? 'complete' : ''} ${row.ended === undefined ? 'open' : ''} ${live ? 'live' : ''}`} style={{ left: `${position(row.started)}%`, width: `${width(row.started, row.ended ?? now)}%` }} />}
    {row.resolved !== undefined && <i className="trace-result-tick" title={`結果確定 ${traceMs(row.resolved - run.origin)} ms`} style={{ left: `${position(row.resolved)}%` }} />}
    {start === undefined && <span className="trace-unknown">開始記録なし</span>}
    <span className="trace-track-state">{status}</span>
  </button></div>;
}
export default function ExecutionTimeline({ connection, blocked, visible }: Props) {
  const recorder = useExecutionTrace(connection, blocked), [view, setView] = useState<'live' | 'a' | 'b' | 'compare'>('live'), [expanded, setExpanded] = useState(false), [transferIssue,setTransferIssue]=useState<string|null>(null);
  const file = useRef<HTMLInputElement>(null), importSlot = useRef<{slot:0|1;version:number}>({slot:0,version:0});
  const active = view === 'a' ? recorder.frozen[0]?.trace : view === 'b' ? recorder.frozen[1]?.trace : recorder.trace;
  function download() {
    if (!active || blocked) return;let url:string|undefined;
    try {url=URL.createObjectURL(new Blob([exportTrace(active)],{type:'application/json'}));const link=document.createElement('a');link.href=url;link.download='DIYIME2-execution-trace.json';link.click();setTransferIssue(null);}
    catch {setTransferIssue('記録を保存できません。記録の容量と形式を確認してください。');}
    finally {if(url){const resource=url;setTimeout(()=>URL.revokeObjectURL(resource),0);}}
  }
  const imported = view === 'a' ? recorder.frozen[0]?.imported : view === 'b' ? recorder.frozen[1]?.imported : false;
  return <section hidden={!visible} className={`execution-timeline ${expanded ? 'expanded' : ''}`} aria-label="実行タイムライン">
    <header className="trace-toolbar"><strong>実行タイムライン</strong><span role="status">{blocked ? '保護中・記録消去' : recorder.issue ? '記録状態未確認' : recorder.trace.recording ? '記録中' : '記録OFF'}</span><button type="button" onClick={() => setExpanded(v => !v)}>{expanded ? '高さを戻す' : '広げる'}</button>
      <button type="button" disabled={blocked || !connection || recorder.busy} onClick={() => { setView('live');void recorder.command(recorder.trace.recording ? 'stop' : 'start'); }}>{recorder.trace.recording ? '記録を停止' : '記録を開始'}</button>
      <button type="button" disabled={blocked || !connection?.startDetailed || recorder.busy || recorder.trace.recording} onClick={() => { setView('live');void recorder.command('startDetailed'); }}>詳細記録を開始</button>
      <button type="button" disabled={blocked || recorder.busy || !connection} onClick={() => void recorder.command('clear')}>記録を消去</button>
      {recorder.issue && <button type="button" disabled={blocked || !connection || recorder.busy} onClick={recorder.retry}>記録を再取得</button>}
    </header>
    {blocked ? <p className="trace-empty">保護のためライブ記録・固定記録を消去しました。解除しても記録は自動再開しません。</p> : <>
      {!connection && <p className="trace-hint">実行結果タブでグラフを確認し、承認すると新しい処理を記録できます。</p>}
      <div className="trace-tools"><label>表示 <select aria-label="タイムラインの表示" value={view} onChange={e => setView(e.target.value as typeof view)}><option value="live">ライブ</option><option value="a" disabled={!recorder.frozen[0]}>固定 A</option><option value="b" disabled={!recorder.frozen[1]}>固定 B</option><option value="compare" disabled={!recorder.frozen[0] || !recorder.frozen[1]}>A / B 比較</option></select></label>
        {([0, 1] as const).map(slot => <button key={slot} type="button" disabled={!recorder.trace.recordingId} onClick={() => { recorder.freeze(slot);setView(slot ? 'b' : 'a'); }}>ライブを{slot ? 'B' : 'A'}に固定</button>)}
        <button type="button" disabled={!active?.recordingId || view === 'compare'} onClick={download}>表示記録を保存</button>
        {([0, 1] as const).map(slot => <button key={slot} type="button" onClick={() => { importSlot.current = {slot,version:recorder.importVersion()};file.current?.click(); }}>{slot ? 'B' : 'A'}へ読込</button>)}
        <input ref={file} type="file" accept="application/json,.json" aria-label="実行記録を読み込む" hidden onChange={e => { const item = e.target.files?.[0], choice = importSlot.current;e.target.value = '';if (item) void recorder.load(item, choice.slot,choice.version).then(loaded => {if(loaded)setView(choice.slot ? 'b' : 'a');}); }} />
      </div>
      {(recorder.issue||transferIssue) && <p className="trace-gap" role="alert">{recorder.issue??transferIssue}</p>}
      <p className="trace-hint">固定・比較・読込は表示だけを操作します。処理の停止や再実行はしません。本文を保存せず、直近60秒を有界で保持します。番号は記録内の別名です。</p>
      <div className="trace-legend"><span className="dependency">依存待ち</span><span className="waiting">実行枠待ち</span><span className="executing">呼出中・終了確認待ち</span><span className="complete">呼出終了確認</span><span className="result">結果確定</span></div>
      {view === 'compare' ? <>{recorder.frozen.map((f, i) => f && <Chart key={`${i}/${f.trace.recordingId}`} trace={f.trace} live={false} title={`固定 ${i ? 'B' : 'A'}${f.imported ? '（読込・表示専用）' : ''}`} />)}<p className="trace-hint">AとBはそれぞれの実行受理を起点に表示します。別の記録内の番号を同じ工程とは自動判定しません。</p></> : active && <Chart key={`${view}/${active.recordingId}`} trace={active} live={view === 'live'} title={view === 'live' ? 'ライブ' : `固定 ${view.toUpperCase()}${imported ? '（読込・表示専用）' : ''}`} />}
      <p className="trace-hint">記録開始前からの実行は遡って記録しません。初回応答はHostの送信完了で、画面描画やIME反映の時刻ではありません。</p>
    </>}
  </section>;
}
