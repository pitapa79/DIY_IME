import { writeFileSync } from 'node:fs';
import { it, expect } from 'vitest';
import { traceConnections } from '../src/graphReachability';
import type { Definition, Graph } from '../src/model';
const port={id:'out',dataType:{id:'text-packet',version:1},language:null,root:'any' as const};
const definitions:Definition[]=[{kind:'bench',version:1,inputs:[{port:{...port,id:'in'},required:false}],outputs:[port],configSchema:{},defaultConfig:{}}];
function graph(n:number,shape:'chain'|'star'):Graph{
  return {version:1,nodes:Array.from({length:n},(_,i)=>({id:`n${i}`,kind:'bench',version:1})),edges:Array.from({length:n-1},(_,i)=>({from:{node:`n${shape==='chain'?i:0}`,port:'out'},to:{node:`n${i+1}`,port:'in'}}))};
}
it.skipIf(process.env.DIY_IME2_BENCHMARK!=='1')('reports optional reachability timings without machine-dependent pass thresholds',()=>{
  const rows: Record<string, unknown>[]=[];
  const sizes=process.env.DIY_IME2_BENCHMARK_LARGE==='1'?[32,128,512,2048,10000]:[32,128,512];
  for(const shape of ['chain','star'] as const) for(const n of sizes){
    const g=graph(n,shape);traceConnections(g,definitions,'n0');const samples:number[]=[];let materialized=0;
    for(let run=0;run<5;run++){const before=performance.now();const result=traceConnections(g,definitions,'n0');samples.push(performance.now()-before);expect(result.reached).toHaveLength(n-1);expect(result.connectionsResolved).toBe(true);if(run===4){const paths=performance.now();for(const item of result.reached.slice(0,100)) void item.path.length;materialized=performance.now()-paths;}}
    samples.sort((a,b)=>a-b);rows.push({shape,n,medianMs:samples[2],pessimisticSampleMs:samples[4],first100PathsMs:materialized,heapUsedMiB:process.memoryUsage().heapUsed/1048576});
  }
  if(process.env.DIY_IME2_BENCHMARK_OUTPUT) writeFileSync(process.env.DIY_IME2_BENCHMARK_OUTPUT,JSON.stringify({node:process.version,rows},null,2));
});
