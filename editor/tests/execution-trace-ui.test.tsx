// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { capture } from './support/executionTrace';
import { emptyTrace, exportTrace, type TracePage } from '../src/executionTrace';
import { useExecutionTrace, type TraceConnection } from '../src/useExecutionTrace';
import ExecutionTimeline from '../src/ExecutionTimeline';
import WorkerTraceDetails from '../src/WorkerTraceDetails';
import { traceRuns } from '../src/executionTrace';
afterEach(cleanup);
it('shows Worker-local durations and missing exits without using the Host axis',()=>{
 const t=capture();t.events.push({...t.events[2],sequence:'10',atUs:'110000',event:'worker-interval',worker:{instance:1,phase:'compile',startedUs:'15000',finishedUs:'37000'}},{...t.events[2],sequence:'11',atUs:'110001',event:'worker-interval',worker:{instance:2,phase:'guest-call',startedUs:'40000'}});
 const row=traceRuns(t)[0].rows[0];render(<WorkerTraceDetails row={row}/>);
 expect(screen.getByLabelText('Worker内部の相対区間').textContent).toContain('40–退出未取得');
 expect(screen.getByText(/Hostの軸とは同期していません/)).toBeTruthy();
 fireEvent.change(screen.getByLabelText('Workerの起動を選択'),{target:{value:'1'}});
 expect(screen.getByLabelText('Worker内部の相対区間').textContent).toContain('15–37 ms（22 ms）');
});
it('starts detailed recording only on an explicit action and fences its late reply on protection',async()=>{
 const c={...connection(),startDetailed:vi.fn(async()=>capture())};
 const ui=render(<ExecutionTimeline connection={c} blocked={false} visible/>);
 expect(c.startDetailed).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('button',{name:'詳細記録を開始'}));
 await waitFor(()=>expect(c.startDetailed).toHaveBeenCalledTimes(1));expect(c.start).not.toHaveBeenCalled();
 ui.unmount();let finish!:(p:TracePage)=>void;c.startDetailed.mockImplementation(()=>new Promise(r=>{finish=r;}));
 const h=renderHook(({blocked})=>useExecutionTrace(c,blocked),{initialProps:{blocked:false}});let pending!:Promise<void>;
 act(()=>{pending=h.result.current.command('startDetailed');});h.rerender({blocked:true});
 await act(async()=>{finish(capture());await pending;});expect(h.result.current.trace).toEqual(emptyTrace());
 h.rerender({blocked:false});expect(c.startDetailed).toHaveBeenCalledTimes(2);expect(c.read).toHaveBeenCalledTimes(1);
});
function connection(){const t=capture();return {scope:{},start:vi.fn(async()=>({...t,events:[],firstSequence:'0',nextSequence:'0'})),read:vi.fn(async()=>t),stop:vi.fn(async()=>({...t,recording:false})),clear:vi.fn(async()=>emptyTrace())} satisfies TraceConnection;}
it('never polls or starts recording by default',async()=>{const c=connection();const h=renderHook(()=>useExecutionTrace(c,false));await act(async()=>{});expect(h.result.current.trace.recording).toBe(false);expect(c.start).not.toHaveBeenCalled();expect(c.read).not.toHaveBeenCalled();});
it('freezes display without stopping or cancelling live recording',async()=>{const c=connection();const h=renderHook(()=>useExecutionTrace(c,false));await act(async()=>{await h.result.current.command('start');});await waitFor(()=>expect(h.result.current.trace.events).toHaveLength(9));act(()=>h.result.current.freeze(0));expect(h.result.current.frozen[0]?.trace.events).toHaveLength(9);expect(h.result.current.trace.recording).toBe(true);expect(c.stop).not.toHaveBeenCalled();expect(c.clear).not.toHaveBeenCalled();});
it('clears live and frozen data on protection and rejects a late read without auto resume',async()=>{const c=connection();let complete!:(p:TracePage)=>void;c.read.mockImplementationOnce(async()=>capture()).mockImplementation(()=>new Promise(r=>{complete=r;}));const h=renderHook(({blocked})=>useExecutionTrace(c,blocked),{initialProps:{blocked:false}});await act(async()=>{await h.result.current.command('start');});await waitFor(()=>expect(h.result.current.trace.events).toHaveLength(9));act(()=>h.result.current.freeze(0));await waitFor(()=>expect(c.read).toHaveBeenCalledTimes(2));h.rerender({blocked:true});expect(h.result.current.trace).toEqual(emptyTrace());expect(h.result.current.frozen).toEqual([null,null]);await act(async()=>complete(capture()));expect(h.result.current.trace).toEqual(emptyTrace());const calls=c.read.mock.calls.length;h.rerender({blocked:false});await act(async()=>{});expect(c.read).toHaveBeenCalledTimes(calls);expect(c.start).toHaveBeenCalledTimes(1);});
it('imports are inert and an old file selection cannot populate a protected generation',async()=>{const c=connection();let finish!:(s:string)=>void;const h=renderHook(({blocked})=>useExecutionTrace(c,blocked),{initialProps:{blocked:false}});const version=h.result.current.importVersion();const file={size:2000,text:()=>new Promise<string>(r=>{finish=r;})} as File;let loaded!:Promise<boolean>;act(()=>{loaded=h.result.current.load(file,0,version);});h.rerender({blocked:true});await act(async()=>{finish(exportTrace(capture()));expect(await loaded).toBe(false);});expect(h.result.current.frozen).toEqual([null,null]);h.rerender({blocked:false});await act(async()=>{expect(await h.result.current.load({size:2000,text:async()=>exportTrace(capture())} as File,0,version)).toBe(false);});await act(async()=>{expect(await h.result.current.load({size:2000,text:async()=>exportTrace(capture())} as File,1)).toBe(true);});expect(h.result.current.frozen[1]?.imported).toBe(true);expect(c.start).not.toHaveBeenCalled();expect(c.read).not.toHaveBeenCalled();expect(c.stop).not.toHaveBeenCalled();});
it('draws Host first-response separately from the long call and expands details on the right',async()=>{const c=connection();const ui=render(<ExecutionTimeline connection={c} blocked={false} visible/>);fireEvent.click(screen.getByRole('button',{name:'記録を開始'}));await waitFor(()=>expect(screen.getByLabelText('初回応答（Host公開） 11 ms')).toBeTruthy());fireEvent.click(screen.getByRole('button',{name:/の詳細：/}));expect(screen.getByLabelText('選択した工程の詳細').textContent).toContain('100 ms');expect(ui.container.querySelector('.trace-layout.with-detail')).toBeTruthy();fireEvent.click(screen.getByRole('button',{name:'ライブをAに固定'}));expect(c.stop).not.toHaveBeenCalled();fireEvent.click(screen.getByRole('button',{name:'ライブをBに固定'}));fireEvent.change(screen.getByLabelText('タイムラインの表示'),{target:{value:'compare'}});expect(screen.getByLabelText('固定 A')).toBeTruthy();expect(screen.getByLabelText('固定 B')).toBeTruthy();ui.rerender(<ExecutionTimeline connection={null} blocked visible/>);expect(screen.queryByLabelText('選択した工程の詳細')).toBeNull();expect(screen.queryByLabelText('固定 A')).toBeNull();});
