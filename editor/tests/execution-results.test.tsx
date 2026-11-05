// @vitest-environment jsdom
import {cleanup, fireEvent, render, screen} from '@testing-library/react';
import {afterEach, expect, it, vi} from 'vitest';
import ExecutionResults from '../src/ExecutionResults';
import type {ExecutionSnapshot} from '../src/executionModel';
import type {Graph} from '../src/model';
afterEach(cleanup);
const graph:Graph={version:1,nodes:[{id:'hiragana',kind:'text.romaji-to-hiragana',version:1},{id:'result',kind:'text.stringify',version:1}],edges:[{from:{node:'hiragana',port:'out'},to:{node:'result',port:'in'}}]};
function snapshot():ExecutionSnapshot{return {key:{sessionEpoch:'0123456789abcdef0123456789abcdef',planVersion:'1',runId:'1',inputRevision:1,deliveryGeneration:'1'},state:'terminal',current:true,eventSeq:'2',elapsedMs:1,pollAfterMs:1,physical:{running:0,cancelRequested:0,stalled:0,queued:0},result:{reason:'completed',delivery:'complete',targets:[{node:'hiragana',port:'pending'},{node:'result',port:'out'}],nodes:[{nodeId:'hiragana',status:'executed',diagnostic:null},{nodeId:'result',status:'executed',diagnostic:null}],ports:[{endpoint:{node:'hiragana',port:'pending'},value:{status:'data',packet:{version:1,root:{type:'text',text:''}}}},{endpoint:{node:'result',port:'out'},value:{status:'data',packet:{version:1,root:{type:'text',text:'降水'}}}}],causes:[],recoveries:[]}};}
it('initially displays the final output while keeping an explicit auxiliary-port choice',()=>{
 const value=snapshot();const ui=render(<ExecutionResults graph={graph} snapshot={value} allowed onClearConnection={vi.fn()}/>);
 expect(screen.getByTestId('v3-output-value').textContent).toBe('降水');
 fireEvent.change(screen.getByLabelText('非同期の表示する出力'),{target:{value:JSON.stringify(['hiragana','pending'])}});
 expect(screen.getByTestId('v3-output-value').textContent).toBe('（空文字列）');
 ui.rerender(<ExecutionResults graph={graph} snapshot={structuredClone(value)} allowed onClearConnection={vi.fn()}/>);
 expect(screen.getByTestId('v3-output-value').textContent).toBe('（空文字列）');
});
it('honors an explicitly selected connection instead of substituting the final output',()=>{
 render(<ExecutionResults graph={graph} snapshot={snapshot()} allowed connection={{from:{node:'hiragana',port:'pending'},to:{node:'inspect',port:'in'}}} onClearConnection={vi.fn()}/>);
 expect(screen.getByTestId('v3-output-value').textContent).toBe('（空文字列）');
});
it('does not replace a requested auxiliary output with an unrequested final output',()=>{
 const value=snapshot();value.result!.targets=[{node:'hiragana',port:'pending'}];
 render(<ExecutionResults graph={graph} snapshot={value} allowed onClearConnection={vi.fn()}/>);
 expect(screen.getByTestId('v3-output-value').textContent).toBe('（空文字列）');
});
it('shows final NoOutput rather than displaying another port as successful data',()=>{
 const value=snapshot();value.result!.ports[1].value={status:'no-output'};
 render(<ExecutionResults graph={graph} snapshot={value} allowed onClearConnection={vi.fn()}/>);
 expect(screen.getByText('この出力はNoOutputです。')).toBeTruthy();expect(screen.queryByTestId('v3-output-value')).toBeNull();
});
