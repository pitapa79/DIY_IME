import {benchmark} from './benchmark-windows-application.mjs';
import {mkdirSync,writeFileSync,readFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import {resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

export async function comparePairedWindowsTrace(folder,output){
 mkdirSync(output);
 const driver={files:Object.fromEntries(['./compare-paired-windows-trace.mjs','./benchmark-windows-application.mjs','./trace-phase-profile.mjs','../editor/src/executionTrace.ts'].map(path=>[path,createHash('sha256').update(readFileSync(new URL(path,import.meta.url))).digest('hex')]))};
 writeFileSync(resolve(output,'driver-provenance.json'),JSON.stringify(driver,null,2)+'\n',{flag:'wx'});
 const report=await benchmark({folder,samples:100,warmups:8,pollMs:5,trace:0,pairedGroups:3,progressEvery:10,
  onPair:pair=>writeFileSync(resolve(output,`group-${pair.group}-pair-${pair.pair}.json`),JSON.stringify(pair,null,2)+'\n',{flag:'wx'})});
 writeFileSync(resolve(output,'complete-report.json'),JSON.stringify({...report,driver},null,2)+'\n',{flag:'wx'});
 return {groups:report.groups.length,pairs:report.pairs.length,samples:report.pairs.length*2,groupsSummary:report.groups};
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 if(process.argv.length!==4){console.error('Usage: node scripts/compare-paired-windows-trace.mjs BUILT_FOLDER NEW_OUTPUT_DIRECTORY');process.exitCode=1;}
 else{try{console.log(JSON.stringify(await comparePairedWindowsTrace(resolve(process.argv[2]),resolve(process.argv[3]))));}catch{console.error('Paired comparison incomplete; completed pair evidence retained. No complete report emitted.');process.exitCode=1;}}
}
