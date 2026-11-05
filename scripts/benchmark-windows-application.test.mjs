import {test} from 'node:test';
import assert from 'node:assert/strict';
import {summarize,physicallyEnded,verifySample,benchmarkOptions,traceConfiguration} from './benchmark-windows-application.mjs';
import {PhaseProfiler,mergePhaseSummaries} from './trace-phase-profile.mjs';
import {emptyTrace} from '../editor/src/executionTrace.ts';

function tracePage(events,dropped=0){return {...emptyTrace(),recordingId:'a'.repeat(32),nowUs:'100000',firstSequence:String(events[0]?.sequence??0),nextSequence:String(events.at(-1)?.sequence??0),events,dropped};}
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
