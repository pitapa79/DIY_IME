import {benchmark} from './benchmark-windows-application.mjs';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';

// A fresh output directory prevents a partial rerun from replacing earlier evidence.
export async function compareWindowsTrace(folder,output){
 mkdirSync(output);
 const orders=[[0,1,2],[2,0,1],[1,2,0]],groups=[];
 let provenance,environment;
 for(let group=0;group<orders.length;group++){
  const runs=[];
  for(const mode of orders[group]){
   console.error(`Starting group ${group+1}/3 mode ${mode} (100 samples, 8 warmups)`);
   const report=await benchmark({folder,samples:100,warmups:8,pollMs:5,trace:mode,progressEvery:10});
   const file=`group-${group+1}-mode-${mode}.json`;
   writeFileSync(resolve(output,file),JSON.stringify(report,null,2)+'\n',{flag:'wx'});
   if(provenance){assert.deepEqual(report.provenance,provenance);assert.deepEqual(report.environment,environment);}
   else{provenance=report.provenance;environment=report.environment;}
   assert.equal(report.diagnostics.mode,['off','basic','host-detailed'][mode]);
   assert.equal(report.samples.length,100);
   assert.equal(report.warmups.length,8);
   assert.equal(report.diagnostics.recordings,mode===0?0:109);
   if(mode===2)assert(report.diagnostics.hostIntervals>0&&report.diagnostics.phaseSummary.worker.compile?.completed>0);
   runs.push({mode:report.diagnostics.mode,summary:report.summary,diagnostics:report.diagnostics,file});
   console.error(`Completed group ${group+1}/3 mode ${mode}: P95 ${report.summary.p95Ms.toFixed(3)} ms, dropped ${report.diagnostics.dropped}`);
  }
  const off=runs.find(r=>r.mode==='off').summary.p95Ms;
  assert(Number.isFinite(off)&&off>0);
  groups.push({group:group+1,order:orders[group].map(m=>['off','basic','host-detailed'][m]),runs,p95IncreasePercent:Object.fromEntries(runs.filter(r=>r.mode!=='off').map(r=>[r.mode,(r.summary.p95Ms/off-1)*100]))});
  // Each completed group is a separate immutable checkpoint; failures retain raw evidence.
  writeFileSync(resolve(output,`summary-through-group-${group+1}.json`),JSON.stringify({formatVersion:1,scope:'small bundled test-dictionary graph; not Reference IME or TSF',provenance,environment,groups},null,2)+'\n',{flag:'wx'});
 }
 return {groups:groups.length,runs:9,samples:900};
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 if(process.argv.length!==4){console.error('Usage: node scripts/compare-windows-trace.mjs BUILT_FOLDER NEW_OUTPUT_DIRECTORY');process.exitCode=1;}
 else{try{console.log(JSON.stringify(await compareWindowsTrace(resolve(process.argv[2]),resolve(process.argv[3]))));}catch{console.error('Comparison incomplete; existing evidence retained. No complete comparison emitted.');process.exitCode=1;}}
}
