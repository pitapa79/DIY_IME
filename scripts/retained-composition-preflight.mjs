import {readFileSync,writeFileSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {retainedCompositionCases} from './retained-composition-cases.mjs';
import {retainedComposition,fixtureDefinition} from './retained-composition-graphs.mjs';
const [officialArg,outputArg]=process.argv.slice(2);
if(!officialArg||!outputArg)throw Error('OFFICIAL OUTPUT required');
const official=JSON.parse(readFileSync(join(resolve(officialArg),'demo.identity.manifest.json'),'utf8')).node;
const cases=retainedCompositionCases().map((spec,index)=>{
  const graph=retainedComposition(spec.count,spec.layout);
  return {...spec,name:`${index}-${spec.mode}-${spec.count}-${spec.layout}-${spec.cadence}`,
    graph:graph.graph,targets:graph.targets,
    definitions:[...new Set(graph.kinds)].map(kind=>kind==='demo.identity'?official:fixtureDefinition(kind))};
});
writeFileSync(resolve(outputArg),JSON.stringify(cases),{flag:'wx'});
