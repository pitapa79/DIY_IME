import { useState } from 'react';
import { traceMs, workerPhaseLabels, type TraceRow, type WorkerInterval } from './executionTrace';

/** Worker-local intervals are deliberately separate from the Host timeline axis. */
export default function WorkerTraceDetails({ row }: { row: TraceRow }) {
  const [chosen,setChosen]=useState<number|null>(null);
  const groups=new Map<number,WorkerInterval[]>();
  for(const e of row.events)if(e.worker){const list=groups.get(e.worker.instance)??[];list.push(e.worker);groups.set(e.worker.instance,list);}
  if(!groups.size)return null;
  const ids=[...groups.keys()].slice(-100),instance=chosen!==null&&ids.includes(chosen)?chosen:ids.at(-1)!;
  const intervals=groups.get(instance)!;
  const maximum=intervals.reduce((m,w)=>{const at=BigInt(w.finishedUs??w.startedUs);return at>m?at:m;},1n);
  const position=(at:string)=>Number(BigInt(at))*100/Number(maximum);
  return <section className="worker-trace-details" aria-label="Worker内部の相対区間">
    <h5>Worker内部（相対時刻）</h5>
    <label>Workerの起動 <select aria-label="Workerの起動を選択" value={instance} onChange={e=>setChosen(Number(e.target.value))}>{ids.map(id=><option key={id} value={id}>#{id}</option>)}</select></label>
    {groups.size>100&&<p>直近100起動を表示しています。保存記録には保持中の全区間が含まれます。</p>}
    <p>Workerの計測開始が0 ms。実終了後に取得し、Hostの軸とは同期していません。退出は成功の判定ではありません。</p>
    <ul>{intervals.map(w=><li key={w.phase}>
      <strong>{workerPhaseLabels[w.phase]}</strong>
      <div className="worker-trace-track" aria-hidden="true"><i className={w.finishedUs===undefined?'incomplete':''} style={{left:`${Math.min(99.5,position(w.startedUs))}%`,width:`${w.finishedUs===undefined ? .5 : Math.max(.5,position(w.finishedUs)-position(w.startedUs))}%`}}/></div>
      <span>{traceMs(BigInt(w.startedUs))}–{w.finishedUs===undefined?'退出未取得':traceMs(BigInt(w.finishedUs))} ms{w.finishedUs!==undefined&&`（${traceMs(BigInt(w.finishedUs)-BigInt(w.startedUs))} ms）`}</span>
    </li>)}</ul>
  </section>;
}
