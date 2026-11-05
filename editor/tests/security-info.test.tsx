// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import SecurityInfo from '../src/SecurityInfo';
import type { Definition, Graph } from '../src/model';
afterEach(cleanup);
const port={id:'out',dataType:{id:'text-packet',version:1},language:null,root:'any' as const};
const definitions: Definition[]=[{kind:'mock',version:1,inputs:[{port:{...port,id:'in'},required:false}],outputs:[port],configSchema:{},defaultConfig:{}}];
const graph: Graph={version:1,nodes:[{id:'a',kind:'mock',version:1},{id:'b',kind:'mock',version:1}],edges:[{from:{node:'a',port:'out'},to:{node:'b',port:'in'}}]};
const props={graph,definitions,remember:false,stored:true,onClose:vi.fn()};
it('shows current limitations and static port paths without permission or protection controls',()=>{
  render(<SecurityInfo {...props}/>);
  expect(screen.getByRole('dialog',{name:'入力と実行の扱い'})).toBeTruthy();
  expect(screen.getByText('このEditorでは未対応です。')).toBeTruthy();
  expect(screen.getByText('送信先・権限・危険度は未判定です。')).toBeTruthy();
  expect(screen.getByText('a.out → b.in')).toBeTruthy();
  expect(screen.queryByRole('checkbox')).toBeNull();
});
it('never lets graph configuration claim trusted permission or protected status',()=>{
  const g=structuredClone(graph);g.nodes[0].config={trusted:true,permissions:['*'],protected:true,text:'secret-config-canary'};
  render(<SecurityInfo {...props} graph={g}/>);
  expect(screen.getByText('このEditorでは未対応です。')).toBeTruthy();
  expect(screen.queryByText('secret-config-canary')).toBeNull();
  expect(screen.queryByText('*')).toBeNull();
});
it('keeps unknown definitions unresolved and treats markup-like IDs as text',()=>{
  const g: Graph={version:1,nodes:[{id:'<img src=x onerror=canary>',kind:'untrusted-claim',version:1}],edges:[]};
  const {container}=render(<SecurityInfo {...props} graph={g} definitions={[]}/>);
  expect(screen.getByText('未解決の接続があります。表示した経路だけで判断できません。')).toBeTruthy();
  expect(container.querySelector('img')).toBeNull();
  expect(screen.getByRole('option').textContent).toBe('<img src=x onerror=canary>');
});
it('recomputes after graph edits and source selection without mutating graph',()=>{
  const before=JSON.stringify(graph);const view=render(<SecurityInfo {...props}/>);
  fireEvent.change(screen.getByLabelText('接続経路の解析元'),{target:{value:'b'}});
  expect(screen.queryByText('a.out → b.in')).toBeNull();
  view.rerender(<SecurityInfo {...props} graph={{...graph,nodes:[],edges:[]}}/>);
  expect(screen.getByText('グラフにノードがありません。')).toBeTruthy();expect(JSON.stringify(graph)).toBe(before);
});
it('reports actual persistence selection and deletion failure rather than claiming no saved data',()=>{
  const view=render(<SecurityInfo {...props} remember/>);
  expect(screen.getByText('選択により、このブラウザーへ保存します。')).toBeTruthy();
  view.rerender(<SecurityInfo {...props} stored={false}/>);
  expect(screen.getByText('保存設定の反映に失敗しています。保存済み入力が残っている可能性があります。')).toBeTruthy();
});
it('supports closing by button, backdrop and Escape',()=>{
  const close=vi.fn();const view=render(<SecurityInfo {...props} onClose={close}/>);
  fireEvent.keyDown(screen.getByRole('button',{name:'閉じる'}),{key:'Escape'});expect(close).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button',{name:'閉じる'}));expect(close).toHaveBeenCalledTimes(2);
  fireEvent.click(view.container.firstChild!);expect(close).toHaveBeenCalledTimes(3);
});
it('leaves large graph analysis unresolved without limiting graph execution',()=>{
  const g: Graph={version:1,nodes:[{id:'a',kind:'mock',version:1}],edges:Array.from({length:20001},()=>({from:{node:'a',port:'out'},to:{node:'a',port:'in'}}))};
  render(<SecurityInfo {...props} graph={g}/>);
  expect(screen.getByText('接続が多いため、この画面では経路を解析していません。')).toBeTruthy();
  expect(screen.getByText('送信先・権限・危険度は未判定です。')).toBeTruthy();
  expect(g.edges).toHaveLength(20001);
});
it('analyses a graph beyond the old display budget and caps only rendered paths',()=>{
  const g: Graph={version:1,nodes:Array.from({length:202},(_,i)=>({id:`n${i}`,kind:'mock',version:1})),edges:Array.from({length:201},(_,i)=>({from:{node:'n0',port:'out'},to:{node:`n${i+1}`,port:'in'}}))};
  const view=render(<SecurityInfo {...props} graph={g}/>);
  expect(screen.queryByText('接続が多いため、この画面では経路を解析していません。')).toBeNull();
  expect(screen.getByText('到達先201件のうち先頭100件を表示しています。')).toBeTruthy();
  expect(view.container.querySelectorAll('.security-paths > li')).toHaveLength(100);
});
it('explains that fixed graph settings are autosaved independently of runtime input consent',()=>{
  render(<SecurityInfo {...props}/>);
  expect(screen.getByText('実行時入力の保存')).toBeTruthy();
  expect(screen.getByText('実行時入力はページ内だけに保持します。')).toBeTruthy();
  expect(screen.getByText('グラフの固定文字列・設定は下書きとして自動保存されます。実行時入力の保存設定とは別です。')).toBeTruthy();
});
