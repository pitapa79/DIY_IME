import {decodeTracePage} from '../editor/src/executionTrace.ts';

function duration(target, phase, elapsed) {
 const item=target[phase]??={completed:0,incomplete:0};
 if(elapsed===undefined){item.incomplete++;return;}
 item.completed++;item.totalUs=(BigInt(item.totalUs??'0')+elapsed).toString();
 item.minUs=(item.minUs===undefined||elapsed<BigInt(item.minUs)?elapsed:BigInt(item.minUs)).toString();
 item.maxUs=(item.maxUs===undefined||elapsed>BigInt(item.maxUs)?elapsed:BigInt(item.maxUs)).toString();
}
/** Merge only completed recording summaries; aliases never cross recording boundaries. */
export function mergePhaseSummaries(target, source) {
 if(target.hostCoverage!==source.hostCoverage){target.hostCoverage='incomplete';target.host={};}
 for(const domain of ['host','worker']){
  if(domain==='host'&&target.hostCoverage!=='complete')continue;
  for(const [phase,item] of Object.entries(source[domain])){
   const out=target[domain][phase]??={completed:0,incomplete:0};
   out.completed+=item.completed;out.incomplete+=item.incomplete;
   if(item.totalUs!==undefined){
    out.totalUs=(BigInt(out.totalUs??'0')+BigInt(item.totalUs)).toString();
    out.minUs=(out.minUs===undefined||BigInt(item.minUs)<BigInt(out.minUs)?item.minUs:out.minUs);
    out.maxUs=(out.maxUs===undefined||BigInt(item.maxUs)>BigInt(out.maxUs)?item.maxUs:out.maxUs);
   }
  }
 }
 return target;
}
/** Independent clock domains. Totals are received intervals, never wall-time shares. */
export class PhaseProfiler {
 #pending=new Map();
 #detailed;
 #recordingId;
 constructor(mode){this.#detailed=mode==='host-detailed';this.summary={hostCoverage:this.#detailed?'complete':'disabled',workerCoverage:'received-intervals-only',host:{},worker:{}};}
 #invalidateHost(){this.summary.hostCoverage='incomplete';this.summary.host={};this.#pending.clear();}
 observe(raw){
  const page=decodeTracePage(raw);
  if(page.recordingId===null||this.#recordingId!==undefined&&this.#recordingId!==page.recordingId)throw Error('trace-recording-changed');
  this.#recordingId=page.recordingId;
  if(page.dropped>0&&this.summary.hostCoverage==='complete')this.#invalidateHost();
  for(const e of page.events){
   const phase=/^(host-request|worker-staging|worker-process|worker-cleanup|broker-exchange)-(started|finished)$/.exec(e.event);
   if(!this.#detailed&&(phase||e.worker))throw Error('unexpected-detailed-data');
   if(e.worker){const w=e.worker;duration(this.summary.worker,w.phase,w.finishedUs===undefined?undefined:BigInt(w.finishedUs)-BigInt(w.startedUs));}
   if(this.summary.hostCoverage!=='complete')continue;
   if(!phase)continue;
   const key=`${e.run}/${e.node}/${phase[1]}`,at=BigInt(e.atUs);
   if(phase[2]==='started'){
    if(this.#pending.has(key)){this.#invalidateHost();continue;}
    this.#pending.set(key,at);
   }else{
    const start=this.#pending.get(key);
    if(start===undefined||at<start){this.#invalidateHost();continue;}
    duration(this.summary.host,phase[1],at-start);this.#pending.delete(key);
   }
  }
  return page;
 }
 finish(){if(this.#pending.size)this.#invalidateHost();return this.summary;}
}
