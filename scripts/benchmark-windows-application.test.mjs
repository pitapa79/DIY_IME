import {test} from 'node:test';
import assert from 'node:assert/strict';
import {summarize,physicallyEnded,verifySample,benchmarkOptions,traceConfiguration,pairedSchedule,makeSyntheticDictionary} from './benchmark-windows-application.mjs';
import {PhaseProfiler,mergePhaseSummaries} from './trace-phase-profile.mjs';
import {measureHostInput} from './host-input-scenarios.mjs';
import {emptyTrace} from '../editor/src/executionTrace.ts';

function tracePage(events,dropped=0){return {...emptyTrace(),recordingId:'a'.repeat(32),nowUs:'100000',firstSequence:String(events[0]?.sequence??0),nextSequence:String(events.at(-1)?.sequence??0),events,dropped};}
test('Host input counts bad results and refuses stale payloads after a newer revision', async () => {
 let latest = 0;
 const measure = leak => measureHostInput({
  submit: async () => ++latest,
  read: async key => ({current: key === latest, result: key === latest || leak ? {reason: 'failed'} : null}),
  ended: () => true,
  verify: () => { throw new Error('incorrect fixture'); },
  summarize, pollMs: 5, rounds: 1,
 });
 const scenarios = await measure(false);
 assert.deepEqual(scenarios.map(scenario => scenario.intervalMs), [null, 100, 20]);
 assert.equal(scenarios[0].failed, 7);
 for (const scenario of scenarios) {
  assert.equal(scenario.attempts, 7);
  assert.equal(scenario.correct, 0);
  assert.equal(scenario.failed + scenario.superseded, 7);
  assert.equal(scenario.firstCorrectResponse, null);
 }
 await assert.rejects(measure(true), /stale-host-result/);
});
test('synthetic dictionary preserves the ranked seed and fits the actual broker limits', () => {
 const seed = Buffer.from('reading\tsurface\tweight\r\nこうすい\t降水\t90\r\nこうすい\t香水\t30\r\nこうすい\t洪水\t90\r\n');
 const config = benchmarkOptions(['bundle', '--synthetic-rows', '10000']);
 const bytes = makeSyntheticDictionary(seed, config.syntheticRows);
 const [header, ...entries] = bytes.toString('utf8').trimEnd().split('\n');
 assert.equal(header, 'reading\tsurface\tweight');
 assert.deepEqual(entries.slice(0, 3), ['こうすい\t降水\t90', 'こうすい\t香水\t30', 'こうすい\t洪水\t90']);
 assert.equal(entries.length, 10_000);
 const filler = entries.slice(3).map(line => line.split('\t')[0]);
 assert.equal(new Set(filler).size, 9_997);
 assert(bytes.length <= 1024 * 1024);
 assert.throws(() => makeSyntheticDictionary(seed, 10_001), /synthetic-dictionary-rows/);
});
test('paired comparison balances order while holding the case fixed within each adjacent pair',()=>{
 const schedule=pairedSchedule(3,100);assert.equal(schedule.length,300);
 for(let group=1;group<=3;group++){
  const entries=schedule.filter(p=>p.group===group);assert.equal(entries.length,100);
  assert.equal(entries.filter(p=>p.modes[0]===0).length,50);
  assert.equal(entries.filter(p=>p.modes[0]===1).length,50);
  assert.deepEqual(entries.map(p=>p.pair),Array.from({length:100},(_,i)=>i+1));
  for(const entry of entries){assert.deepEqual([...entry.modes].sort(),[0,1]);assert.equal(entry.caseIndex,(entry.pair-1)%6);}
  for(let caseIndex=0;caseIndex<6;caseIndex++){
   const sameCase=entries.filter(p=>p.caseIndex===caseIndex);
   assert(Math.abs(sameCase.filter(p=>p.modes[0]===0).length-sameCase.filter(p=>p.modes[0]===1).length)<=1);
   for(let i=1;i<sameCase.length;i++)assert.notEqual(sameCase[i].modes[0],sameCase[i-1].modes[0]);
  }
 }
 assert.notDeepEqual(schedule[0].modes,schedule[100].modes);
 for(const [groups,samples] of [[0,100],[4,100],[3,99],[3,0],[3,Infinity]])assert.throws(()=>pairedSchedule(groups,samples),/paired-count/);
});
function event(sequence,at,event,node='1:1',worker){return {sequence:String(sequence),atUs:String(at),run:1,plan:1,node,nodeKind:'test.worker',nodeVersion:1,event,...(worker?{worker}:{})};}
test('profiles concurrent Host intervals across pages and Worker-local clocks independently',()=>{
 const p=new PhaseProfiler('host-detailed');
 p.observe(tracePage([event(1,10,'worker-process-started'),event(2,20,'worker-process-started','1:2')]));
 p.observe(tracePage([event(3,50,'worker-process-finished'),event(4,80,'worker-process-finished','1:2'),event(5,90,'worker-interval','1:1',{instance:1,phase:'compile',startedUs:'10000000',finishedUs:'10001000'})]));
 const result=p.finish();assert.equal(result.hostCoverage,'complete');
 assert.deepEqual(result.host['worker-process'],{completed:2,incomplete:0,totalUs:'100',minUs:'40',maxUs:'60'});
 assert.deepEqual(result.worker.compile,{completed:1,incomplete:0,totalUs:'1000',minUs:'1000',maxUs:'1000'});
});
test('never turns missing exits into zero duration or retained uncertain Host totals',()=>{
 const p=new PhaseProfiler('host-detailed');
 p.observe(tracePage([event(1,10,'worker-process-started'),event(2,20,'worker-process-finished'),event(3,30,'worker-interval','1:1',{instance:1,phase:'guest-call',startedUs:'12'})]));
 assert.deepEqual(p.summary.worker['guest-call'],{completed:0,incomplete:1});
 p.observe(tracePage([],1));assert.equal(p.finish().hostCoverage,'incomplete');assert.deepEqual(p.summary.host,{});
 const open=new PhaseProfiler('host-detailed');open.observe(tracePage([event(1,10,'worker-process-started')]));assert.equal(open.finish().hostCoverage,'incomplete');assert.deepEqual(open.summary.host,{});
 const wrong=new PhaseProfiler('host-detailed');wrong.observe(tracePage([event(1,10,'worker-process-finished')]));assert.equal(wrong.finish().hostCoverage,'incomplete');
});
test('keeps received durations exact beyond safe JS integers and rejects private fields',()=>{
 const p=new PhaseProfiler('host-detailed');p.observe(tracePage([event(1,1,'worker-interval','1:1',{instance:1,phase:'compile',startedUs:'0',finishedUs:'18446744073709551615'}),event(2,2,'worker-interval','1:1',{instance:2,phase:'compile',startedUs:'0',finishedUs:'1'})]));
 assert.equal(p.finish().worker.compile.totalUs,'18446744073709551616');
 assert.throws(()=>p.observe(tracePage([event(3,3,'worker-interval','1:1',{instance:3,phase:'compile',startedUs:'0',body:'PRIVATE'})])));
 assert.deepEqual(new PhaseProfiler('off').finish().host,{});
 assert.throws(()=>new PhaseProfiler('basic').observe(tracePage([event(1,10,'worker-process-started')])),/unexpected-detailed-data/);
});
test('refuses to aggregate different recording generations or a cleared recording',()=>{
 const p=new PhaseProfiler('host-detailed');p.observe(tracePage([event(1,1,'worker-process-started')]));
 assert.throws(()=>p.observe({...tracePage([]),recordingId:'b'.repeat(32)}),/trace-recording-changed/);
 assert.throws(()=>new PhaseProfiler('basic').observe(emptyTrace()),/trace-recording-changed/);
});
test('aggregates completed recording summaries without pairing aliases across generations',()=>{
 const total=new PhaseProfiler('host-detailed').summary;
 for(const id of ['a','b']){
  const p=new PhaseProfiler('host-detailed');
  p.observe({...tracePage([event(1,10,'worker-process-started'),event(2,30,'worker-process-finished'),event(3,35,'worker-interval','1:1',{instance:1,phase:'compile',startedUs:'2',finishedUs:id==='a'?'12':'22'})]),recordingId:id.repeat(32)});
  mergePhaseSummaries(total,p.finish());
 }
 assert.deepEqual(total.host['worker-process'],{completed:2,incomplete:0,totalUs:'40',minUs:'20',maxUs:'20'});
 assert.deepEqual(total.worker.compile,{completed:2,incomplete:0,totalUs:'30',minUs:'10',maxUs:'20'});
 const partial=new PhaseProfiler('host-detailed');partial.observe(tracePage([event(1,1,'worker-process-started'),event(2,2,'worker-interval','1:1',{instance:1,phase:'compile',startedUs:'0'})]));
 mergePhaseSummaries(total,partial.finish());
 assert.equal(total.hostCoverage,'incomplete');assert.deepEqual(total.host,{});assert.equal(total.worker.compile.incomplete,1);assert.equal(total.worker.compile.totalUs,'30');
 mergePhaseSummaries(total,new PhaseProfiler('host-detailed').finish());assert.deepEqual(total.host,{});
});

test('trace modes preserve OFF/basic and explicitly select Host detail without fallback',()=>{
 assert.deepEqual(traceConfiguration(),{mode:'off',operation:null});
 assert.deepEqual(traceConfiguration(1),{mode:'basic',operation:{method:'traceStart'}});
 assert.deepEqual(traceConfiguration(2),{mode:'host-detailed',operation:{method:'traceStartDetailed'}});
 for(const mode of [null,true,-1,3,'2',NaN])assert.throws(()=>traceConfiguration(mode),/trace-mode/);
 assert.equal(benchmarkOptions(['bundle']).trace,0);
 for(const mode of ['0','1','2'])assert.equal(benchmarkOptions(['bundle','--trace',mode]).trace,Number(mode));
 for(const args of [['bundle','--trace','3'],['bundle','--trace','detail'],['bundle','--trace'],['bundle','--trace','1','--trace','2']])assert.throws(()=>benchmarkOptions(args));
 assert.equal(benchmarkOptions(['bundle']).progressEvery,0);assert.equal(benchmarkOptions(['bundle','--progress-every','10']).progressEvery,10);
 for(const value of ['-1','10001','true'])assert.throws(()=>benchmarkOptions(['bundle','--progress-every',value]));
});
function sample(){return {state:'terminal',current:true,physical:{running:0,cancelRequested:0,stalled:0,queued:0},result:{reason:'completed',delivery:'complete',targets:[{node:'result',port:'out'}],causes:[],ports:[{endpoint:{node:'result',port:'out'},value:{status:'data',packet:{version:1,root:{type:'text',text:''}}}}]}};}
test('nearest-rank tail retains slow observations and leaves raw order intact',()=>{
 const values=[...Array(98).fill(2),300,100];assert.deepEqual(summarize(values),{count:100,minMs:2,p50Ms:2,p95Ms:2,p99Ms:100,maxMs:300});assert.deepEqual(values.slice(-2),[300,100]);
 for(const invalid of [[],[NaN],[Infinity],[-1]])assert.throws(()=>summarize(invalid),/invalid-samples/);
});
test('logical completion does not settle outstanding physical ownership',()=>{
 for(const key of ['running','cancelRequested','stalled','queued']){const value=sample();value.physical[key]=1;assert.equal(physicallyEnded(value),false);assert.throws(()=>verifySample(value,''),/not-ended/);}
 const value=sample();value.state='running';assert.equal(physicallyEnded(value),false);
 for(const physical of [{},{running:0},null,{running:0,cancelRequested:0,stalled:-1,queued:0}])assert.throws(()=>physicallyEnded({...sample(),physical}),/invalid-physical-state/);
});
test('empty Data counts only with exact current successful output and target',()=>{
 verifySample(sample(),'');
 for(const change of [s=>s.current=false,s=>s.result.delivery='recovered',s=>s.result.reason='cancelled',s=>s.result.targets=[],s=>s.result.causes.push({code:'failure'}),s=>s.result.ports[0].value={status:'no-output'},s=>s.result.ports[0].value={status:'error'},s=>s.result.ports[0].value.packet.root.text='stale',s=>s.result.ports.push(structuredClone(s.result.ports[0]))]){const value=sample();change(value);assert.throws(()=>verifySample(value,''));}
});
