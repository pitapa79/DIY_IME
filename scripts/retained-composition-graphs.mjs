// Synthetic, typed graph specifications for the real normal-Host benchmark.
import assert from 'node:assert/strict';
export const fixtureKinds=['fixture.wasm.chain','fixture.wasm.chain-alt','demo.identity','fixture.wasm.join'];
export function retainedComposition(count,layout='serial') {
  assert([1,2,4,8,16,20].includes(count));
  assert(['serial','parallel','join','unused'].includes(layout));
  assert(layout!=='join'||count>=4,'join needs two branches, join and terminal official node');
  const ids=Array.from({length:count},(_,i)=>`stage${i}`);
  const kindAt=i=>layout==='join'&&i===count-2?fixtureKinds[3]
    :count>=3&&i===count-1?fixtureKinds[2]:fixtureKinds[i%2];
  const nodes=[{id:'input',kind:'input.text',version:1},...ids.map((id,i)=>({
    id,kind:kindAt(i),version:1,...(kindAt(i)===fixtureKinds[2]?{}:
      {config:{mode:kindAt(i)===fixtureKinds[3]?'join':layout==='join'&&i===1?'echo':'forward'}}),
  }))];
  const edge=(source,destination,port='in')=>({from:{node:source,port:'out'},to:{node:destination,port}});
  let edges;
  if(layout==='join') {
    const branchCount=count-2;
    edges=ids.slice(0,branchCount).map((id,i)=>edge(i<2?'input':ids[i-2],id));
    // Two true dependency chains reconverge; official Any-output remains terminal.
    edges.push(edge(ids[branchCount-2],ids[count-2]),
      edge(ids[branchCount-1],ids[count-2],'other'),edge(ids[count-2],ids[count-1]));
  } else edges=ids.map((id,i)=>edge(layout==='serial'&&i?ids[i-1]:'input',id));
  const executedIds=layout==='unused'?[ids[0]]:ids;
  const targets=(layout==='parallel'?ids:layout==='unused'?[ids[0]]:[ids.at(-1)])
    .map(node=>({node,port:'out'}));
  return {layout,ids,executedIds,unexecutedIds:ids.filter(id=>!executedIds.includes(id)),
    kinds:ids.map((_,i)=>kindAt(i)),graph:{version:1,nodes,edges},targets,
    expected:text=>layout==='join'?{kind:'distinct-join',left:text,rightKind:fixtureKinds[1],rightInput:text}:text,
    verify(actual,wanted) {
      if(layout!=='join') {assert.equal(actual,wanted);return;}
      const prefix=wanted.left+'|';
      assert.equal(typeof actual,'string');assert(actual.startsWith(prefix),'left branch was changed or swapped');
      const right=JSON.parse(actual.slice(prefix.length));
      assert.equal(right.apiVersion,1);assert.equal(right.node.kind,wanted.rightKind);
      assert.equal(right.operation.config.mode,'echo');
      assert.equal(right.operation.inputs.in.packet.root.text,wanted.rightInput);
    },
    // Lifetime reservations and concurrently active Core callbacks both charge
    // the finite budget. Report this profile; never silently compare budgets.
    maxPhysicalJobs:['parallel','join'].includes(layout)?Math.max(22,2*count+2):22};
}

export function fixtureDefinition(kind) {
  assert(fixtureKinds.includes(kind)&&kind!==fixtureKinds[2]);
  const port={id:'in',dataType:{id:'text-packet',version:1},language:'ja-Latn',root:'text'};
  const inputs=[{port,required:true}];
  if(kind===fixtureKinds[3])inputs.push({port:{...port,id:'other'},required:true});
  return {kind,version:1,inputs,outputs:[{...port,id:'out'}],
    configSchema:{type:'object'},defaultConfig:{mode:'echo'}};
}
