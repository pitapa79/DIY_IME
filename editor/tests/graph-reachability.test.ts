import { expect, it } from 'vitest';
import { connectionError, definitionFor, edgeId } from '../src/model';
import { traceConnections } from '../src/graphReachability';
import type { Definition, Graph, GraphEdge } from '../src/model';
const port = { id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'any' as const };
const definitions: Definition[] = [{ kind: 'mock', version: 1, inputs: [{ port: { ...port, id: 'in' }, required: false }], outputs: [port, { ...port, id: 'other' }], configSchema: {}, defaultConfig: {} }];
const edge = (a: string, b: string, output = 'out'): GraphEdge => ({ from: { node: a, port: output }, to: { node: b, port: 'in' } });
const graph = (ids: string[], edges: GraphEdge[]): Graph => ({ version: 1, nodes: ids.map(id => ({ id, kind: 'mock', version: 1 })), edges });
it('finds the exact connected chain, preserves port paths and does not claim safety', () => {
  const g = graph(['a','b','c','isolated'], [edge('a','b'),edge('b','c')]);
  const before = JSON.stringify(g); const r = traceConnections(g, definitions, 'a');
  expect(r.reached).toEqual([{ node:'b', path:[g.edges[0]] },{ node:'c', path:g.edges }]);
  expect(r.connectionsResolved).toBe(true); expect(r.risk).toBe('not-assessed');
  expect(JSON.stringify(g)).toBe(before);
});
it('conservatively follows all outputs rather than predicting runtime branches', () => {
  const g = graph(['a','b','yes','no'], [edge('a','b'),edge('b','yes'),edge('b','no','other')]);
  expect(traceConnections(g,definitions,'a').reached.map(x=>x.node).sort()).toEqual(['b','no','yes']);
});
it('reports unknown definitions and ports instead of treating missing paths as safe', () => {
  const g = graph(['a','b','c'],[edge('a','b'),edge('b','c')]); g.nodes[1].kind='unresolved';
  const r = traceConnections(g,definitions,'a');
  expect(r.connectionsResolved).toBe(false); expect(r.issues.length).toBeGreaterThan(0);
  expect(r.risk).toBe('not-assessed'); expect(r.reached).toEqual([]);
  const bad = graph(['a','b'],[edge('a','b','missing')]);
  expect(traceConnections(bad,definitions,'a').connectionsResolved).toBe(false);
});
it('rejects incompatible types and duplicate input connections while retaining independent paths', () => {
  const g=graph(['a','b','c'],[edge('a','c'),edge('b','c')]);
  expect(traceConnections(g,definitions,'a').connectionsResolved).toBe(false);
  const defs=structuredClone(definitions); defs[0].inputs[0].port.dataType={ id: 'text-packet', version: 2 };
  expect(traceConnections(graph(['a','b'],[edge('a','b')]),defs,'a').reached).toEqual([]);
});
it('terminates on cycles and self loops and marks unresolved structure', () => {
  for(const edges of [[edge('a','b'),edge('b','a')],[edge('a','a')]]) {
    const r=traceConnections(graph(['a','b'],edges),definitions,'a');
    expect(r.connectionsResolved).toBe(false); expect(r.risk).toBe('not-assessed');
  }
});
it('handles missing sources, duplicate node IDs and unsupported versions without certainty', () => {
  for(const g of [graph(['a','a'],[]),{...graph(['a'],[]),version:2}]) {
    const r=traceConnections(g,definitions,'a'); expect(r.connectionsResolved).toBe(false); expect(r.reached).toEqual([]);
  }
  expect(traceConnections(graph(['a'],[]),definitions,'missing').connectionsResolved).toBe(false);
});
it('handles special identifiers as literal map keys and is deterministic for reordered edges', () => {
  const g=graph(['__proto__','constructor','<script>canary</script>'],[edge('__proto__','constructor'),edge('constructor','<script>canary</script>')]);
  expect(traceConnections(g,definitions,'__proto__').reached.map(x=>x.node)).toEqual(['constructor','<script>canary</script>']);
  expect(traceConnections({...g,edges:[...g.edges].reverse()},definitions,'__proto__').reached).toEqual(traceConnections(g,definitions,'__proto__').reached);
});
it('does not label disconnected nodes or a graph without external metadata safe', () => {
  const r=traceConnections(graph(['a','b'],[]),definitions,'a');
  expect(r.reached).toEqual([]); expect(r.risk).toBe('not-assessed'); expect(r.connectionsResolved).toBe(true);
});

// Independent compatibility oracle using the existing editor connection API.
function referenceTrace(g: Graph, defs: Definition[], source: string) {
  const issues = new Set<string>(), nodes = new Map(g.nodes.map(n => [n.id,n]));
  if(nodes.size!==g.nodes.length) issues.add('ノードIDが重複しています。');
  if(g.version!==1) issues.add('グラフの版を解決できません。');
  if(!nodes.has(source)) issues.add('解析元のノードがありません。');
  for(const node of g.nodes) if(!definitionFor(defs,node)) issues.add(`${node.id}: ノード宣言を取得できません。`);
  const edges=g.edges.filter((edge,index)=>{const error=connectionError({...g,edges:g.edges.filter((_,i)=>i!==index)},defs,edge);if(error) issues.add(`${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}: ${error}`);return !error;}).toSorted((a,b)=>edgeId(a)<edgeId(b)?-1:edgeId(a)>edgeId(b)?1:0);
  const reached:{node:string;path:GraphEdge[]}[]=[];
  if(nodes.size===g.nodes.length&&nodes.has(source)&&g.version===1){
    const seen=new Set([source]),pending=[{node:source,path:[] as GraphEdge[]}];
    for(let i=0;i<pending.length;i++) for(const edge of edges){if(edge.from.node!==pending[i].node||seen.has(edge.to.node))continue;seen.add(edge.to.node);const next={node:edge.to.node,path:[...pending[i].path,edge]};reached.push(next);pending.push(next);}
  }
  return {source,reached,issues:[...issues],connectionsResolved:issues.size===0,risk:'not-assessed'};
}
it('matches the existing connection validator on every directed three-node graph and malformed variants',()=>{
  const possible=Array.from({length:9},(_,i)=>edge(['a','b','c'][Math.floor(i/3)],['a','b','c'][i%3]));
  for(let mask=0;mask<512;mask++){
    const g=graph(['a','b','c'],possible.filter((_,i)=>(mask&(1<<i))!==0));
    for(const source of ['a','b','c'])expect(traceConnections(g,definitions,source)).toEqual(referenceTrace(g,definitions,source));
  }
  const base=graph(['a','b','c'],[edge('a','b'),edge('b','c')]);
  const cases:[Graph,Definition[]][]=[];
  for(const change of ['missing-node','missing-definition','missing-port','duplicate-input','duplicate-node','type','language','root','cycle-through-invalid-port','version']){
    const g=structuredClone(base),defs=structuredClone(definitions);
    if(change==='missing-node')g.nodes.pop();
    if(change==='missing-definition')g.nodes[1].kind='unknown';
    if(change==='missing-port')g.edges[0].from.port='absent';
    if(change==='duplicate-input')g.edges.push(structuredClone(g.edges[0]));
    if(change==='duplicate-node')g.nodes.push(structuredClone(g.nodes[0]));
    if(change==='type')defs[0].inputs[0].port.dataType={id:'other',version:2};
    if(change==='language')defs[0].inputs[0].port.language='ja';
    if(change==='root')defs[0].inputs[0].port.root='sequence';
    if(change==='cycle-through-invalid-port')g.edges.push(edge('c','a','missing'));
    if(change==='version')g.version=2;
    cases.push([g,defs]);
  }
  for(const [g,defs]of cases)for(const source of ['a','missing'])expect(traceConnections(g,defs,source)).toEqual(referenceTrace(g,defs,source));
});
it('resolves a ten-thousand-node chain and its final complete path without recursive stack growth',()=>{
  const n=10000,g=graph(Array.from({length:n},(_,i)=>`n${i}`),Array.from({length:n-1},(_,i)=>edge(`n${i}`,`n${i+1}`)));
  const result=traceConnections(g,definitions,'n0');expect(result.connectionsResolved).toBe(true);expect(result.reached).toHaveLength(n-1);
  const last=result.reached.at(-1)!;expect(last.node).toBe('n9999');expect(last.path).toHaveLength(n-1);expect(last.path[0]).toEqual(g.edges[0]);expect(last.path.at(-1)).toEqual(g.edges.at(-1));
});
it('rejects a ten-thousand-node cycle without recursion and preserves the unknown-risk result',()=>{
  const n=10000,g=graph(Array.from({length:n},(_,i)=>`n${i}`),Array.from({length:n},(_,i)=>edge(`n${i}`,`n${(i+1)%n}`)));
  const result=traceConnections(g,definitions,'n0');expect(result.connectionsResolved).toBe(false);expect(result.reached).toEqual([]);expect(result.issues).toHaveLength(n);expect(result.risk).toBe('not-assessed');
});

it('keeps the BFS parent chain finite if callers mutate or remove graph edges before reading a lazy path',()=>{
  const g=graph(['a','b','c'],[edge('a','b'),edge('b','c')]);
  const links=[...g.edges];const result=traceConnections(g,definitions,'a');
  // Mutating this live edge must not turn c's traversal into c -> c forever.
  links[1].from.node='c';g.edges=[];
  const path=result.reached.find(item=>item.node==='c')!.path;
  expect(path).toHaveLength(2);expect(path[0]).toBe(links[0]);expect(path[1]).toBe(links[1]);
  expect(result.reached.find(item=>item.node==='c')!.path).toBe(path);
});
