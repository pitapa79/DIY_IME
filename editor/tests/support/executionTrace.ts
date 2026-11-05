import { emptyTrace, type TraceKind, type TracePage } from '../../src/executionTrace';
export function capture(recording = true): TracePage {
 const node={node:'1:1',nodeKind:'test.slow',nodeVersion:1};
 const entries: [TraceKind,number,boolean,boolean?][]=[['run-accepted',0,false],['dependency-wait',0,true],['node-ready',500,true],['node-started',1000,true,true],['node-resolved',10000,true],['core-result-ready',10000,false],['host-response-published',11000,false],['call-ended',100000,true,true],['physical-ended',100100,false]];
 return { ...emptyTrace(),recordingId:'a'.repeat(32),recording,nowUs:'120000',firstSequence:'1',nextSequence:'9',events:entries.map(([event,at,hasNode,call],i)=>({sequence:String(i+1),atUs:String(at),run:1,input:1,plan:1,event,...(hasNode?node:{}),...(call?{call:1}:{})})) };
}
