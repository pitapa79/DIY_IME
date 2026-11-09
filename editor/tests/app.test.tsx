// @vitest-environment jsdom
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import App from '../src/App';
import { ApiError } from '../src/api';
import { invokeCore } from '../server/bridge';
import type { Definition, Graph, Report, RuntimeInput, SessionResult, SessionState } from '../src/model';

const mocks = vi.hoisted(() => ({ definitions: vi.fn(), sample: vi.fn(), fitView: vi.fn(), fitBounds: vi.fn(), getNodesBounds: vi.fn(), openSession: vi.fn(), closeSession: vi.fn(), sessionStatus: vi.fn(), sessionValidate: vi.fn(), prepare: vi.fn(), activate: vi.fn(), runActive: vi.fn(), rollback: vi.fn() }));
vi.mock('../src/api', async importOriginal => ({ ...await importOriginal<typeof import('../src/api')>(), api: mocks }));
vi.mock('@xyflow/react', () => ({
  useReactFlow: () => mocks, useNodesInitialized: () => true,
  ReactFlow: ({ nodes, edges, nodeTypes, onNodeClick, onEdgeClick, onPaneClick, children }: any) => <div>{edges.map((edge: any) => <button key={edge.id} aria-label={edge.ariaLabel} onClick={event => onEdgeClick(event, edge)} />)}<button aria-label="選択を解除" onClick={onPaneClick} />{nodes.map((node: any) => {
    const Component = nodeTypes[node.type]; return <div key={node.id}><button aria-label={`選択 ${node.id}`} onClick={event => onNodeClick(event, node)}>選択 {node.id}</button><Component data={node.data} selected={node.selected} /></div>;
  })}{children}</div>,
  Handle: () => <span />, Background: () => null, Controls: () => null,
  BackgroundVariant: { Dots: 'dots' }, MarkerType: { ArrowClosed: 'arrowclosed' }, Position: { Left: 'left', Right: 'right' },
}));
// Rendering/state tests use captured reports; real processing is checked by the actual Wasm app script.
const capturedReports = JSON.parse(readFileSync(resolve(process.cwd(), 'tests/fixtures/legacy-reports.json'), 'utf8'));
function capturedLegacyReport(graph: Graph): Report {
  const key = graph.nodes.some(node => node.kind === 'text.romaji-to-hiragana') ? 'practical' : (graph.nodes[0].config as { text?: unknown } | undefined)?.text === 'honjitunokousuirituha30%desu' ? 'weather-negative' : 'weather';
  return structuredClone(capturedReports[key]);
}
const experimentalCatalog: { kind: string }[] = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/experimental-catalog.json'), 'utf8'));
const sample: Graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/hello.json'), 'utf8'));
const weatherSample: Graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/weather.json'), 'utf8'));
const runtimeSample: Graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/runtime-input.json'), 'utf8'));
let definitions: Definition[], report: Report, state: SessionState, prior: Graph | null, token: number, runId: number;
const empty = (): SessionState => ({ revision: 0, active: null, prepared: null, canRollback: false });
const reply = <T,>(result: T): SessionResult<T> => ({ state: structuredClone(state), result });
beforeAll(async () => { definitions = JSON.parse(readFileSync(resolve(process.cwd(), '../docs/node-migration-inventory.json'), 'utf8')).nodes.filter((entry: { registered: boolean }) => entry.registered).map((entry: { definition: Definition }) => entry.definition); report = await invokeCore('run', JSON.stringify(sample), undefined, null, { core: resolve(process.cwd(), '../target/debug', process.platform === 'win32' ? 'diy-ime2-test-host.exe' : 'diy-ime2-test-host'), root: resolve(process.cwd(), '..') }) as Report; });
beforeEach(() => {
  localStorage.clear(); vi.resetAllMocks(); state = empty(); prior = null; token = 0; runId = 0;
  mocks.getNodesBounds.mockReturnValue({ x: 80, y: 80, width: 240, height: 260 });
  mocks.definitions.mockResolvedValue(definitions); mocks.sample.mockResolvedValue(structuredClone(sample));
  mocks.openSession.mockImplementation(async () => { state = empty(); return { id: 'test-session', state: structuredClone(state) }; });
  mocks.closeSession.mockResolvedValue(undefined); mocks.sessionStatus.mockImplementation(async () => reply({}));
  mocks.sessionValidate.mockImplementation(async () => reply({ valid: true, order: ['input', 'uppercase', 'result'] }));
  mocks.prepare.mockImplementation(async (_id: string, graph: Graph) => { state = { ...state, revision: state.revision + 1, prepared: { token: String(++token), graph: structuredClone(graph), order: ['input', 'uppercase', 'result'] } }; return reply({}); });
  mocks.activate.mockImplementation(async () => { prior = state.active?.graph ?? null; state = { revision: state.revision + 1, active: { version: (state.active?.version ?? 0) + 1, graph: state.prepared!.graph }, prepared: null, canRollback: !!prior }; return reply({}); });
  mocks.runActive.mockImplementation(async () => reply({ ...structuredClone(report), planVersion: state.active!.version, runId: ++runId }));
  mocks.rollback.mockImplementation(async () => { state = { revision: state.revision + 1, active: { version: state.active!.version + 1, graph: prior! }, prepared: null, canRollback: false }; return reply({}); });
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 1; });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement;
async function boot() { render(<App />); await waitFor(() => expect(button('計画を準備').disabled).toBe(false)); }
async function activate(version = 1) {
  fireEvent.click(button('計画を準備')); await waitFor(() => expect(button('切替').disabled).toBe(false));
  fireEvent.click(button('切替')); await screen.findByText(`稼働中 v${version}`); await waitFor(() => expect(button('実行').disabled).toBe(false));
}
// このヘルパーは実CLIを呼ぶケースにも使う。通信側の10秒上限まで応答を待つ。
async function run() { fireEvent.click(button('実行')); await screen.findByTestId('output-value', {}, { timeout: 10_000 }); await waitFor(() => expect(button('実行').disabled).toBe(false), { timeout: 10_000 }); }
function editInput(text: string) {
  fireEvent.click(button('選択 input')); fireEvent.change(screen.getByLabelText('設定 JSON'), { target: { value: JSON.stringify({ packet: { version: 1, root: { type: 'text', text } } }) } }); fireEvent.click(button('設定を適用'));
}

function runtimeReport(input: RuntimeInput, version = state.active!.version): Report {
  const execution = { status: 'executed' as const, diagnostic: null, outputs: { out: { status: 'data' as const, packet: { version: 1, root: { type: 'text' as const, text: input.text } } } } };
  return { succeeded: true, runId: ++runId, planVersion: version, inputRevision: input.revision, nodes: { input: structuredClone(execution), result: structuredClone(execution) } };
}
function runtimeMocks() {
  mocks.sample.mockResolvedValue(structuredClone(runtimeSample));
  mocks.runActive.mockImplementation(async (_id: string, version: number, input: RuntimeInput) => reply(runtimeReport(input, version)));
}
const pauseAutoRun = (ms = 250) => act(async () => { await new Promise(resolve => setTimeout(resolve, ms)); });
const autoRunToggle = () => screen.getByRole('checkbox', { name: '入力を自動実行' }) as HTMLInputElement;

describe('persistent editor session', () => {
  it('explicitly reconnects exhausted input without losing text, mode, graph or history and cancels queued auto runs', async () => {
    const saved = { revision: Number.MAX_SAFE_INTEGER, text: '𠮷ka', mode: 'finalize' };
    localStorage.setItem('diy-ime2.editor.remember-runtime-input.v1', 'true');
    localStorage.setItem('diy-ime2.editor.runtime-input.v1', JSON.stringify(saved));
    runtimeMocks(); await boot();
    fireEvent.click(button('Input（固定文字列）を追加')); // 編集履歴も再接続とは独立して保持する。
    await activate(); await run();
    const graph = localStorage.getItem('diy-ime2.editor.v1');
    fireEvent.click(autoRunToggle());
    let finish!: (value: { id: string; state: SessionState }) => void;
    mocks.openSession.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    fireEvent.click(button('入力を保持して再接続'));
    expect(autoRunToggle().checked).toBe(false);
    await waitFor(() => expect(mocks.closeSession).toHaveBeenCalledWith('test-session'));
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!)).toEqual(saved);
    await act(async () => { state = empty(); finish({ id: 'recovered-session', state: structuredClone(state) }); });
    await screen.findByText('計画未設定');
    expect(screen.queryByRole('button', { name: '入力を保持して再接続' })).toBeNull();
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!)).toEqual({ ...saved, revision: 1 });
    expect(localStorage.getItem('diy-ime2.editor.v1')).toBe(graph);
    expect(button('元に戻す').disabled).toBe(false);
    expect(button('実行').disabled).toBe(true); expect(screen.queryByTestId('output-value')).toBeNull();
    await pauseAutoRun(); expect(mocks.runActive).toHaveBeenCalledTimes(1);
    expect(mocks.prepare).toHaveBeenCalledTimes(1); expect(mocks.activate).toHaveBeenCalledTimes(1);
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: '𠮷kya' } });
    await activate(); await pauseAutoRun(); expect(mocks.runActive).toHaveBeenCalledTimes(1);
    await run(); expect(mocks.runActive).toHaveBeenLastCalledWith('recovered-session', 1, { revision: 2, text: '𠮷kya', mode: 'finalize' });
  });
  it('keeps an exhausted draft after failed recovery and permits an explicit retry', async () => {
    const saved = { revision: Number.MAX_SAFE_INTEGER, text: 'before\ud800after', mode: 'compose' };
    localStorage.setItem('diy-ime2.editor.remember-runtime-input.v1', 'true');
    localStorage.setItem('diy-ime2.editor.runtime-input.v1', JSON.stringify(saved));
    runtimeMocks(); await boot();
    mocks.openSession.mockRejectedValueOnce(new Error('recovery connection failed'));
    fireEvent.click(button('入力を保持して再接続')); await screen.findByText('recovery connection failed');
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!)).toEqual(saved);
    mocks.openSession.mockResolvedValueOnce({ id: 'retry-session', state: empty() });
    fireEvent.click(button('入力を保持して再接続')); await screen.findByText('計画未設定');
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!)).toEqual({ ...saved, revision: 1 });
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'before𠮷after' } });
    expect((screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement).value).toBe('before𠮷after');
    expect(button('実行').disabled).toBe(true); expect(mocks.runActive).not.toHaveBeenCalled();
  });
  it('does not reconnect exhausted input while a run is in flight', async () => {
    localStorage.setItem('diy-ime2.editor.remember-runtime-input.v1', 'true');
    localStorage.setItem('diy-ime2.editor.runtime-input.v1', JSON.stringify({ revision: Number.MAX_SAFE_INTEGER, text: 'n', mode: 'compose' }));
    runtimeMocks(); await boot(); await activate();
    let finish!: (value: SessionResult<Report>) => void;
    mocks.runActive.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    fireEvent.click(button('実行')); expect(button('入力を保持して再接続').disabled).toBe(true);
    fireEvent.click(button('入力を保持して再接続')); expect(mocks.openSession).toHaveBeenCalledTimes(1);
    await act(async () => finish(reply(runtimeReport({ revision: Number.MAX_SAFE_INTEGER, text: 'n', mode: 'compose' }))));
    await waitFor(() => expect(button('入力を保持して再接続').disabled).toBe(false));
  });
  it('keeps a malformed Unicode draft editable and blocks manual and automatic runs until repaired', async () => {
    const saved = { revision: 7, text: 'prefix\ud800suffix', mode: 'compose' };
    localStorage.setItem('diy-ime2.editor.remember-runtime-input.v1', 'true');
    localStorage.setItem('diy-ime2.editor.runtime-input.v1', JSON.stringify(saved));
    runtimeMocks(); await boot();
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    expect(field.value).toBe(saved.text); expect(field.getAttribute('aria-invalid')).toBe('true');
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!)).toEqual(saved);
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(button('切替').disabled).toBe(false));
    fireEvent.click(button('切替')); await screen.findByText('稼働中 v1');
    expect(button('実行').disabled).toBe(true);
    fireEvent.click(autoRunToggle()); await pauseAutoRun();
    expect(mocks.runActive).not.toHaveBeenCalled();
    fireEvent.change(field, { target: { value: 'prefix𠮷suffix' } });
    await waitFor(() => expect(screen.getByTestId('output-value').textContent).toBe('prefix𠮷suffix'));
    expect(field.getAttribute('aria-invalid')).toBe('false');
    expect(mocks.runActive).toHaveBeenCalledTimes(1);
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 8, text: 'prefix𠮷suffix', mode: 'compose' });
    expect(mocks.prepare).toHaveBeenCalledTimes(1); expect(mocks.activate).toHaveBeenCalledTimes(1);
  });
  it('focuses the selected node without changing the graph, history or active plan', async () => {
    await boot(); await activate();
    const saved = localStorage.getItem('diy-ime2.editor.v1');
    mocks.fitBounds.mockClear(); mocks.getNodesBounds.mockClear();
    fireEvent.click(button('選択に拡大'));
    expect(mocks.getNodesBounds).toHaveBeenLastCalledWith(['input']);
    expect(mocks.fitBounds).toHaveBeenLastCalledWith({ x: 80, y: 80, width: 240, height: 260 }, { padding: 0.25, duration: 220 });
    expect(localStorage.getItem('diy-ime2.editor.v1')).toBe(saved);
    expect(mocks.prepare).toHaveBeenCalledTimes(1); expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect(button('元に戻す').disabled).toBe(true);
    fireEvent.click(button('選択を解除')); expect(button('選択に拡大').disabled).toBe(true);
    fireEvent.click(button('選択 result')); fireEvent.click(button('選択に拡大'));
    expect(mocks.getNodesBounds).toHaveBeenLastCalledWith(['result']);
  });
  it('automatically runs only the latest input after opting in and explicitly activating a runtime plan', async () => {
    runtimeMocks(); await boot();
    expect(autoRunToggle().checked).toBe(false);
    const field = screen.getByLabelText('input 入力テキスト');
    fireEvent.change(field, { target: { value: 'k' } }); await pauseAutoRun();
    expect(mocks.runActive).not.toHaveBeenCalled();
    fireEvent.click(autoRunToggle()); await pauseAutoRun();
    expect(mocks.prepare).not.toHaveBeenCalled(); expect(mocks.activate).not.toHaveBeenCalled();
    expect(mocks.runActive).not.toHaveBeenCalled();
    for (const text of ['ky', 'kya']) fireEvent.change(field, { target: { value: text } });
    await activate();
    await waitFor(() => expect(screen.getByTestId('output-value').textContent).toBe('kya'));
    expect(mocks.runActive).toHaveBeenCalledTimes(1);
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 4, text: 'kya', mode: 'compose' });
    expect(mocks.prepare).toHaveBeenCalledTimes(1); expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.v1')!).graph).toEqual(runtimeSample);
  });
  it('discards an old automatic response and runs just the newest snapshot after it finishes', async () => {
    runtimeMocks(); await boot(); await activate();
    const field = screen.getByLabelText('input 入力テキスト');
    fireEvent.change(field, { target: { value: 'k' } });
    const old = reply(runtimeReport({ revision: 2, text: 'k', mode: 'compose' }));
    let finish!: (value: SessionResult<Report>) => void;
    mocks.runActive.mockReturnValueOnce(new Promise<SessionResult<Report>>(resolve => { finish = resolve; }));
    fireEvent.click(autoRunToggle()); await waitFor(() => expect(mocks.runActive).toHaveBeenCalledTimes(1));
    for (const text of ['ky', 'kya', 'n']) fireEvent.change(field, { target: { value: text } });
    fireEvent.change(screen.getByLabelText('input 入力モード'), { target: { value: 'finalize' } });
    await pauseAutoRun(); expect(mocks.runActive).toHaveBeenCalledTimes(1);
    await act(async () => finish(old));
    expect(screen.queryByTestId('output-value')?.textContent).not.toBe('k');
    await waitFor(() => expect(screen.getByTestId('output-value').textContent).toBe('n'));
    expect(mocks.runActive).toHaveBeenCalledTimes(2);
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 6, text: 'n', mode: 'finalize' });
  });
  it('waits for native composition, prevents manual duplicates, and cancels queued runs when switched OFF', async () => {
    runtimeMocks(); await boot(); await activate(); fireEvent.click(autoRunToggle());
    const field = screen.getByLabelText('input 入力テキスト');
    fireEvent.compositionStart(field); fireEvent.change(field, { target: { value: 'に' } });
    await pauseAutoRun(); expect(mocks.runActive).not.toHaveBeenCalled();
    fireEvent.compositionEnd(field);
    await waitFor(() => expect(screen.getByTestId('output-value').textContent).toBe('に'));
    fireEvent.change(field, { target: { value: 'manual' } }); await run();
    await pauseAutoRun(); expect(mocks.runActive).toHaveBeenCalledTimes(2);
    fireEvent.change(field, { target: { value: 'cancelled' } }); fireEvent.click(autoRunToggle());
    await pauseAutoRun(); expect(mocks.runActive).toHaveBeenCalledTimes(2);
    expect(screen.getByText('入力変更前の結果')).toBeTruthy();
  });
  it('does not retry automatic failures or reactivate a plan after reconnection', async () => {
    runtimeMocks(); await boot(); await activate();
    mocks.runActive.mockRejectedValueOnce(new ApiError('invalid-config', 'automatic run failed', structuredClone(state)));
    fireEvent.click(autoRunToggle()); await screen.findByText('automatic run failed');
    await pauseAutoRun(500); expect(mocks.runActive).toHaveBeenCalledTimes(1);
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'retry with new input' } });
    await waitFor(() => expect(screen.getByTestId('output-value').textContent).toBe('retry with new input'));
    mocks.runActive.mockRejectedValueOnce(new ApiError('session-lost', 'worker terminated'));
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'worker loss' } });
    await screen.findByText('worker terminated'); fireEvent.click(button('再接続'));
    await waitFor(() => expect(button('計画を準備').disabled).toBe(false)); await pauseAutoRun();
    expect(mocks.runActive).toHaveBeenCalledTimes(3); expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect(button('実行').disabled).toBe(true);
  });
  it('does not automatically execute fixed-input graphs', async () => {
    await boot(); await activate(); fireEvent.click(autoRunToggle()); await pauseAutoRun();
    expect(mocks.runActive).not.toHaveBeenCalled();
    await run(); expect(screen.getByTestId('output-value').textContent).toBe('HELLO 𠮷');
  });
  it('runs new snapshots in the same plan and marks cable data stale after text or mode edits', async () => {
    runtimeMocks(); await boot(); await activate();
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    fireEvent.change(field, { target: { value: 'a' } }); await run();
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 2, text: 'a', mode: 'compose' });
    fireEvent.click(button('ケーブル input.out → result.in'));
    fireEvent.change(screen.getByLabelText('input 入力モード'), { target: { value: 'finalize' } });
    expect(screen.getByText('入力変更前の結果')).toBeTruthy();
    fireEvent.change(field, { target: { value: 'ka' } }); await run();
    expect(screen.getByTestId('output-value').textContent).toBe('ka');
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 4, text: 'ka', mode: 'finalize' });
    expect(mocks.prepare).toHaveBeenCalledTimes(1); expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.v1')!).graph).toEqual(runtimeSample);
    fireEvent.change(field, { target: { value: '' } }); await run();
    expect(screen.getByTestId('output-value').textContent).toBe('（空文字列）');
    fireEvent.change(field, { target: { value: '𠮷30%' } }); await run();
    expect(screen.getByTestId('output-value').textContent).toBe('𠮷30%');
    expect(localStorage.getItem('diy-ime2.editor.runtime-input.v1')).toBeNull();
  });
  it('keeps prepared plans and rollback independent of the input buffer', async () => {
    runtimeMocks(); await boot(); await activate();
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(button('切替').disabled).toBe(false));
    const prepared = structuredClone(state.prepared);
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'kya' } });
    expect(state.prepared).toEqual(prepared); expect(button('切替').disabled).toBe(false);
    fireEvent.click(button('切替')); await screen.findByText('稼働中 v2');
    fireEvent.change(screen.getByLabelText('input 入力モード'), { target: { value: 'finalize' } });
    fireEvent.click(button('復帰')); await screen.findByText('稼働中 v3'); await run();
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 3, { revision: 3, text: 'kya', mode: 'finalize' });
  });
  it.each(['text', 'mode'])('discards an in-flight response after the runtime %s changes', async changed => {
    runtimeMocks(); await boot(); await activate();
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'k' } });
    const old = reply(runtimeReport({ revision: 2, text: 'k', mode: 'compose' }));
    let finish!: (value: SessionResult<Report>) => void;
    mocks.runActive.mockReturnValueOnce(new Promise<SessionResult<Report>>(resolve => { finish = resolve; }));
    fireEvent.click(button('実行'));
    if (changed === 'text') fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'kya' } });
    else fireEvent.change(screen.getByLabelText('input 入力モード'), { target: { value: 'finalize' } });
    await act(async () => finish(old));
    expect(screen.queryByTestId('output-value')).toBeNull(); expect(button('実行').disabled).toBe(false);
    await run(); expect(screen.getByTestId('output-value').textContent).toBe(changed === 'text' ? 'kya' : 'k');
  });
  it('keeps input out of graph history and restores only the buffer after reload', async () => {
    runtimeMocks(); await boot(); fireEvent.click(screen.getByLabelText('実行時入力をこのブラウザーに保存')); await activate();
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'saved' } });
    fireEvent.click(button('文字列化を追加')); fireEvent.click(button('元に戻す'));
    expect((screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement).value).toBe('saved');
    fireEvent.click(button('グラフ JSON'));
    expect(JSON.parse((screen.getByLabelText('グラフ JSON') as HTMLTextAreaElement).value)).toEqual(runtimeSample);
    cleanup(); await boot();
    expect((screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement).value).toBe('saved');
    expect(button('実行').disabled).toBe(true); expect(mocks.activate).toHaveBeenCalledTimes(1);
  });
  it('retains the buffer across explicit reconnection and rejects mismatched input revisions', async () => {
    runtimeMocks(); await boot(); await activate();
    fireEvent.change(screen.getByLabelText('input 入力テキスト'), { target: { value: 'kept' } });
    mocks.runActive.mockRejectedValueOnce(new ApiError('session-lost', 'worker terminated'));
    fireEvent.click(button('実行')); await screen.findByText('worker terminated'); fireEvent.click(button('再接続'));
    await waitFor(() => expect(button('計画を準備').disabled).toBe(false));
    expect((screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement).value).toBe('kept');
    await activate(); mocks.runActive.mockResolvedValueOnce(reply(runtimeReport({ revision: 99, text: 'wrong', mode: 'compose' })));
    fireEvent.click(button('実行')); await screen.findByText('実行結果の入力更新番号が一致しません。再実行してください。');
    expect(screen.queryByTestId('output-value')).toBeNull(); await run();
    expect(screen.getByTestId('output-value').textContent).toBe('kept');
  });

  it('requires explicit selection to persist runtime input and erases it when disabled', async () => {
    runtimeMocks(); await boot(); await activate();
    const toggle = screen.getByLabelText('実行時入力をこのブラウザーに保存') as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    fireEvent.change(field, { target: { value: 'storage-canary' } });
    expect(localStorage.getItem('diy-ime2.editor.runtime-input.v1')).toBeNull();
    fireEvent.click(toggle);
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.runtime-input.v1')!).text).toBe('storage-canary');
    expect(screen.getByText('下書きと実行時入力をブラウザーに保存')).toBeTruthy();
    fireEvent.click(toggle);
    expect(localStorage.getItem('diy-ime2.editor.runtime-input.v1')).toBeNull();
    expect(field.value).toBe('storage-canary');
    expect(screen.getByText('グラフの下書きを保存・実行時入力はページ内のみ')).toBeTruthy();
  });

  it('restores only runtime snapshot fields and sends a complete clean request', async () => {
    localStorage.setItem('diy-ime2.editor.remember-runtime-input.v1', 'true');
    localStorage.setItem('diy-ime2.editor.runtime-input.v1', JSON.stringify({ revision: 7, text: 'restored', mode: 'finalize', extra: true }));
    runtimeMocks(); await boot(); await activate(); await run();
    expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1, { revision: 7, text: 'restored', mode: 'finalize' });
    expect((screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement).value).toBe('restored');
  });
  it('edits Input on the canvas and updates the running conversion only after activation', async () => {
    mocks.sample.mockResolvedValue(structuredClone(weatherSample));
    mocks.runActive.mockImplementation(async () => reply({ ...capturedLegacyReport(state.active!.graph), planVersion: state.active!.version }));
    await boot(); await activate(); await run(); expect(screen.getByTestId('output-value').textContent).toBe('本日の降水確率は30%です');
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    fireEvent.focus(field); fireEvent.change(field, { target: { value: 'honjitunokousuirituha30%desu' } });
    expect((screen.getByLabelText('入力テキスト') as HTMLTextAreaElement).value).toBe(field.value);
    expect(button('設定を適用').disabled).toBe(true); expect(button('計画を準備').disabled).toBe(false);
    expect(screen.getByText('編集前の結果')).toBeTruthy();
    expect(JSON.parse(localStorage.getItem('diy-ime2.editor.v1')!).graph.nodes[0].config.text).toBe(field.value);
    fireEvent.keyDown(field, { key: 'Delete' }); expect(screen.getByLabelText('input 入力テキスト')).toBeTruthy();
    fireEvent.blur(field); await run(); expect(screen.getByTestId('output-value').textContent).toBe('本日の降水確率は30%です');
    await activate(2); await run(); expect(screen.getByTestId('output-value').textContent).toBe('本日の降水率は30%です');
  });
  it('undoes a whole inline edit and keeps the inspector draft synchronized', async () => {
    mocks.sample.mockResolvedValue(structuredClone(weatherSample)); await boot();
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    fireEvent.focus(field);
    for (const value of ['a', 'ab', 'abc']) fireEvent.change(field, { target: { value } });
    fireEvent.blur(field); fireEvent.click(button('元に戻す'));
    expect(field.value).toBe('honjitunokousuikakurituha30%desu');
    fireEvent.click(button('やり直す')); expect(field.value).toBe('abc');
    fireEvent.click(button('選択 input'));
    fireEvent.change(screen.getByLabelText('設定 JSON'), { target: { value: JSON.stringify({ mode: 'fixed', text: 'draft' }) } });
    expect(field.value).toBe('draft');
    fireEvent.focus(field); fireEvent.change(field, { target: { value: 'direct' } }); fireEvent.blur(field);
    expect(JSON.parse((screen.getByLabelText('設定 JSON') as HTMLTextAreaElement).value)).toEqual({ mode: 'fixed', text: 'direct' });
    expect(button('設定を適用').disabled).toBe(true); expect(button('計画を準備').disabled).toBe(false);
  });
  it('preserves malformed inspector JSON instead of overwriting it from the inline field', async () => {
    mocks.sample.mockResolvedValue(structuredClone(weatherSample)); await boot();
    const field = screen.getByLabelText('input 入力テキスト') as HTMLTextAreaElement;
    fireEvent.change(screen.getByLabelText('設定 JSON'), { target: { value: '{' } });
    expect(field.disabled).toBe(true); expect(field.value).toBe('honjitunokousuikakurituha30%desu');
    expect(screen.getByText('設定 JSON を確認してください。')).toBeTruthy();
    fireEvent.click(button('戻す')); expect(field.disabled).toBe(false);
    fireEvent.focus(field); fireEvent.change(field, { target: { value: '𠮷'.repeat(4096) } });
    expect(field.getAttribute('aria-invalid')).toBe('false');
    fireEvent.change(field, { target: { value: '𠮷'.repeat(4097) } });
    expect(field.getAttribute('aria-invalid')).toBe('true'); expect(screen.getByText('4096文字以内で入力してください。')).toBeTruthy();
  });
  it('allows direct input for reserved-looking node IDs', async () => {
    const graph = structuredClone(weatherSample); graph.nodes[0].id = '__proto__'; graph.edges[0].from.node = '__proto__';
    mocks.sample.mockResolvedValue(graph); await boot();
    const field = screen.getByLabelText('__proto__ 入力テキスト') as HTMLTextAreaElement;
    expect(field.disabled).toBe(false); expect(field.value).toBe('honjitunokousuikakurituha30%desu');
    fireEvent.focus(field); fireEvent.change(field, { target: { value: 'honjitu' } });
    expect(JSON.parse((screen.getByLabelText('設定 JSON') as HTMLTextAreaElement).value).text).toBe('honjitu');
  });
  it('inspects a cable before and after execution, including candidate trees', async () => {
    await boot(); fireEvent.click(button('グラフ JSON'));
    fireEvent.click(button('ケーブル input.out → uppercase.in'));
    expect(screen.getByText('この接続はまだ実行されていません')).toBeTruthy();
    expect(screen.getByTestId('connection-data').textContent).toContain('input.out → uppercase.in');
    await activate(); await run();
    expect(JSON.parse(screen.getByTestId('output-value').textContent!).type).toBe('sequence');
    expect(screen.getByTestId('output-value').textContent).toContain('hello');
    fireEvent.click(button('ケーブル uppercase.out → result.in'));
    expect(screen.getByTestId('output-value').textContent).toContain('HELLO');
    expect(screen.getByTestId('connection-data').textContent).toContain('uppercase.out → result.in');
    expect(button('選択した接続を削除').disabled).toBe(false);
    fireEvent.change(screen.getByLabelText('表示する出力'), { target: { value: JSON.stringify(['result', 'out']) } });
    expect(screen.getByTestId('output-value').textContent).toBe('HELLO 𠮷');
    expect(screen.queryByTestId('connection-data')).toBeNull();
  });
  it('keeps unapplied graph JSON when inspecting a cable from its tab', async () => {
    await boot(); await activate(); await run(); fireEvent.click(button('グラフ JSON'));
    fireEvent.change(screen.getByLabelText('グラフ JSON'), { target: { value: '{' } });
    fireEvent.click(button('ケーブル input.out → uppercase.in'));
    expect(screen.getByText('編集前の結果')).toBeTruthy(); expect(button('計画を準備').disabled).toBe(true);
    fireEvent.click(button('グラフ JSON'));
    expect((screen.getByLabelText('グラフ JSON') as HTMLTextAreaElement).value).toBe('{');
  });
  it('labels no-output and error cables instead of showing another ports data', async () => {
    await boot(); await activate(); const noOutput = structuredClone(report);
    noOutput.nodes.uppercase.outputs.out = { status: 'no-output' };
    mocks.runActive.mockResolvedValueOnce(reply(noOutput)); fireEvent.click(button('実行'));
    await waitFor(() => expect(button('実行').disabled).toBe(false)); fireEvent.click(button('ケーブル uppercase.out → result.in'));
    expect(screen.getByText('この接続にはデータが出力されませんでした（NoOutput）。')).toBeTruthy();
    expect(screen.queryByTestId('output-value')).toBeNull();
    const failed = structuredClone(report);
    failed.succeeded = false; failed.nodes.uppercase.outputs.out = { status: 'error', diagnostic: { nodeId: 'uppercase', port: 'out', runId: 2, planVersion: 1, error: { code: 'conversion-failed', message: '変換失敗' } } };
    mocks.runActive.mockResolvedValueOnce(reply(failed)); fireEvent.click(button('実行'));
    await screen.findByText(/この接続はエラーのためデータがありません/);
    expect(screen.queryByTestId('output-value')).toBeNull();
  });
  it('does not present an old source output as data that ran through a new cable', async () => {
    await boot(); await activate(); await run();
    fireEvent.click(button('ケーブル uppercase.out → result.in')); fireEvent.click(button('選択した接続を削除'));
    fireEvent.click(button('接続を追加')); fireEvent.change(screen.getByLabelText('入力ノード'), { target: { value: 'result' } });
    fireEvent.click(button('接続する'));
    fireEvent.click(button('ケーブル input.out → result.in'));
    expect(screen.getByText(/この接続はまだ実行されていません/)).toBeTruthy();
    expect(screen.getByText('編集前の結果')).toBeTruthy();
    expect(screen.queryByTestId('output-value')).toBeNull();
  });
  it('shows practical nodes alongside the seven Issue #1 nodes and edits fixed input', async () => {
    const weather: Graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/weather.json'), 'utf8'));
    mocks.sample.mockResolvedValue(weather); await boot();
    expect(mocks.sample).toHaveBeenCalledWith('weather');
    const library = document.querySelectorAll('.library-node'); expect(library).toHaveLength(12 + experimentalCatalog.length);
    expect(button('文字列化（実用）を追加')).toBeTruthy();
    expect(button('ローマ字→ひらがな（実用）を追加')).toBeTruthy();
    expect(screen.queryByRole('button', { name: '大文字に変換を追加' })).toBeNull();
    expect(screen.queryByRole('button', { name: '条件分岐を追加' })).toBeNull();
    expect((screen.getByLabelText('入力テキスト') as HTMLTextAreaElement).value).toBe('honjitunokousuikakurituha30%desu');
    fireEvent.change(screen.getByLabelText('入力テキスト'), { target: { value: 'honjitunokousuirituha30%desu' } });
    expect(button('計画を準備').disabled).toBe(true); fireEvent.click(button('設定を適用'));
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(mocks.prepare).toHaveBeenCalled());
    expect(mocks.prepare.mock.calls[0][1].nodes[0].config).toEqual({ mode: 'fixed', text: 'honjitunokousuirituha30%desu' });
    fireEvent.change(screen.getByLabelText('ノードを検索'), { target: { value: '大文字' } });
    expect(button('大文字・小文字変換を追加')).toBeTruthy();
    expect(document.querySelectorAll('.library-node')).toHaveLength(1);
    expect(screen.queryByRole('button', { name: '大文字に変換を追加' })).toBeNull();
  });
  it('exposes the separate missing-kaku sample', async () => {
    await boot(); fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'weather-negative' } });
    await waitFor(() => expect(mocks.sample).toHaveBeenLastCalledWith('weather-negative'));
  });
  it('loads the dictionary sample and preserves the relative resource name in applied settings', async () => {
    await boot();
    mocks.sample.mockResolvedValue(JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/dictionary-exact.json'), 'utf8')));
    fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'dictionary-exact' } });
    await screen.findByRole('button', { name: '選択 dictionary' });
    expect(mocks.sample).toHaveBeenLastCalledWith('dictionary-exact');
    expect(button('完全一致辞書を追加')).toBeTruthy();
    fireEvent.click(button('選択 dictionary'));
    expect((screen.getByLabelText('辞書ファイル') as HTMLInputElement).value).toBe('dictionary.tsv');
    expect(screen.getByText(/辞書の変更は「計画を準備」/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText('辞書ファイル'), { target: { value: 'local/custom.tsv' } });
    fireEvent.click(button('設定を適用'));
    fireEvent.click(button('計画を準備'));
    await waitFor(() => expect(mocks.prepare).toHaveBeenCalled());
    expect(mocks.prepare.mock.calls.at(-1)?.[1].nodes.find((node: any) => node.id === 'dictionary').config).toEqual({ path: 'local/custom.tsv' });
  });
  it('loads the practical stringify sample and explains that choices must be selected first', async () => {
    await boot();
    mocks.sample.mockResolvedValue(JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/stringify.json'), 'utf8')));
    fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'stringify' } });
    await waitFor(() => expect(mocks.sample).toHaveBeenLastCalledWith('stringify'));
    await screen.findByText('Text / 選択済み Sequence → Text');
    fireEvent.click(button('選択 result'));
    expect(screen.getByText(/Choiceが残っているとエラーになります/)).toBeTruthy();
  });
  it('loads the practical romaji sample with pending output and editable unknown policy', async () => {
    await boot();
    const graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/runtime-romaji.json'), 'utf8'));
    mocks.sample.mockResolvedValue(graph);
    fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'runtime-romaji' } });
    await screen.findByRole('button', { name: '選択 romaji' });
    expect(mocks.sample).toHaveBeenLastCalledWith('runtime-romaji');
    expect(screen.getByLabelText('input 入力テキスト')).toBeTruthy();
    expect(screen.getByText('pending')).toBeTruthy();
    fireEvent.click(button('選択 romaji'));
    fireEvent.change(screen.getByLabelText('未知のローマ字'), { target: { value: 'error' } });
    fireEvent.click(button('設定を適用'));
    fireEvent.click(button('計画を準備'));
    await waitFor(() => expect(mocks.prepare).toHaveBeenCalled());
    expect(mocks.prepare.mock.calls[0][1].nodes[1].config).toEqual({ overrides: [], unknown: 'error' });
  });
  it('loads practical selection and edits its unscored setting without accepting non-finite values', async () => {
    await boot();
    mocks.sample.mockResolvedValue(JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/candidate-selection.json'), 'utf8')));
    fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'candidate-selection' } });
    await screen.findByRole('button', { name: '選択 select' }); fireEvent.click(button('選択 select'));
    expect(button('候補選択（実用）を追加')).toBeTruthy();
    const score = screen.getByRole('textbox', { name: /スコア省略時の評価値/ });
    fireEvent.change(score, { target: { value: 'Infinity' } }); expect(button('設定を適用').disabled).toBe(true);
    fireEvent.change(score, { target: { value: '-5' } }); fireEvent.click(button('設定を適用'));
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(mocks.prepare).toHaveBeenCalled());
    expect(mocks.prepare.mock.calls[0][1].nodes.find((node: { id: string }) => node.id === 'select').config).toEqual({ unscored: -5 });
  });
  it('shows the final sink instead of an unused intermediate pending output in the practical pipeline', async () => {
    const graph = JSON.parse(readFileSync(resolve(process.cwd(), '../examples/graphs/practical-pipeline.json'), 'utf8'));
    graph.nodes[0] = { ...graph.nodes[0], kind: 'e2e.input', config: { mode: 'fixed', text: 'honjitunokousuikakurituha30%desu' } };
    mocks.sample.mockResolvedValue(graph);
    mocks.runActive.mockImplementation(async () => reply({ ...capturedLegacyReport(state.active!.graph), planVersion: state.active!.version }));
    await boot(); await activate(); await run();
    expect((screen.getByLabelText('表示する出力') as HTMLSelectElement).value).toBe(JSON.stringify(['output', 'out']));
    expect(screen.getByTestId('output-value').textContent).toBe('本日の降水確率は30%です');
    fireEvent.change(screen.getByLabelText('表示する出力'), { target: { value: JSON.stringify(['hiragana', 'pending']) } });
    expect(screen.getByTestId('output-value').textContent).toBe('（空文字列）');
  });
  it('requires preparation and explicit activation before running', async () => {
    await boot(); expect(button('実行').disabled).toBe(true);
    fireEvent.click(button('検査')); await screen.findByText('接続の検査が完了しました');
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(button('切替').disabled).toBe(false)); expect(button('実行').disabled).toBe(true);
    fireEvent.click(button('切替')); await screen.findByText('稼働中 v1'); await run();
    expect(mocks.openSession).toHaveBeenCalledTimes(1); expect(mocks.runActive).toHaveBeenCalledWith('test-session', 1);
    expect(screen.getByTestId('output-value').textContent).toBe('HELLO 𠮷');
  });
  it('keeps the old plan runnable while editing and preparing', async () => {
    await boot(); await activate(); await run(); fireEvent.change(screen.getByLabelText('設定 JSON'), { target: { value: '{' } });
    expect(button('計画を準備').disabled).toBe(true); expect(button('設定を適用').disabled).toBe(true); expect(button('実行').disabled).toBe(false);
    fireEvent.change(screen.getByLabelText('設定 JSON'), { target: { value: JSON.stringify({ packet: { version: 1, root: { type: 'text', text: 'updated' } } }) } });
    fireEvent.click(button('設定を適用')); expect(screen.getByText('編集前の結果')).toBeTruthy();
    fireEvent.click(button('計画を準備')); await waitFor(() => expect(button('切替').disabled).toBe(false)); expect(screen.getByText('稼働中 v1')).toBeTruthy();
    await run(); expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1);
    fireEvent.click(button('切替')); await screen.findByText('稼働中 v2'); await run(); expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 2);
  });
  it('removes incident edges and restores them with undo', async () => {
    await boot(); fireEvent.click(button('選択 uppercase')); fireEvent.click(button('ノードを削除'));
    expect(screen.queryByRole('button', { name: '選択 uppercase' })).toBeNull(); fireEvent.click(button('グラフ JSON'));
    expect(JSON.parse((screen.getByLabelText('グラフ JSON') as HTMLTextAreaElement).value).edges).toHaveLength(0);
    fireEvent.click(button('元に戻す')); fireEvent.click(button('グラフ JSON'));
    expect(JSON.parse((screen.getByLabelText('グラフ JSON') as HTMLTextAreaElement).value).edges).toHaveLength(2);
  });
  it('discards an outdated run response without terminating its session', async () => {
    await boot(); await activate(); let finish!: (value: SessionResult<Report>) => void;
    mocks.runActive.mockReturnValueOnce(new Promise<SessionResult<Report>>(resolve => { finish = resolve; }));
    fireEvent.click(button('実行')); fireEvent.click(button('文字列化を追加')); await act(async () => finish(reply({ ...report, planVersion: 1 })));
    expect(screen.queryByTestId('output-value')).toBeNull(); expect(screen.getByText('稼働中 v1')).toBeTruthy(); expect(mocks.closeSession).not.toHaveBeenCalled();
  });
  it('preserves the active plan after a rejected preparation', async () => {
    await boot(); await activate(); editInput('invalid draft'); mocks.prepare.mockRejectedValueOnce(new ApiError('invalid-config', 'invalid-config: input', structuredClone(state)));
    fireEvent.click(button('計画を準備')); await screen.findByText('invalid-config: input'); expect(screen.getByText('稼働中 v1')).toBeTruthy(); expect(button('実行').disabled).toBe(false);
    await run(); expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 1);
  });
  it('cannot activate a preparation whose graph changed while the request was in flight', async () => {
    await boot(); await activate();
    let finish!: (value: SessionResult) => void;
    mocks.prepare.mockReturnValueOnce(new Promise<SessionResult>(resolve => { finish = resolve; }));
    fireEvent.click(button('計画を準備')); editInput('newer edit');
    state = { ...state, revision: state.revision + 1, prepared: { token: '2', graph: structuredClone(sample), order: [] } };
    await act(async () => finish(reply({})));
    expect(button('切替').disabled).toBe(true); expect(button('実行').disabled).toBe(false);
    expect(mocks.activate).toHaveBeenCalledTimes(1);
  });
  it('rolls back the active plan while preserving the editor draft', async () => {
    await boot(); await activate(); editInput('updated'); await activate(2); fireEvent.click(button('復帰')); await screen.findByText('稼働中 v3');
    expect(mocks.rollback).toHaveBeenCalledWith('test-session', 2); expect((screen.getByLabelText('設定 JSON') as HTMLTextAreaElement).value).toContain('updated');
    await run(); expect(mocks.runActive).toHaveBeenLastCalledWith('test-session', 3); fireEvent.click(button('稼働中のグラフを開く'));
    expect((screen.getByLabelText('設定 JSON') as HTMLTextAreaElement).value).toContain('hello');
  });
  it('reconnects explicitly after worker loss and never reactivates automatically', async () => {
    await boot(); await activate(); mocks.runActive.mockRejectedValueOnce(new ApiError('session-lost', 'worker terminated')); fireEvent.click(button('実行')); await screen.findByText('worker terminated');
    expect(button('実行').disabled).toBe(true); fireEvent.click(button('再接続')); await waitFor(() => expect(button('計画を準備').disabled).toBe(false));
    expect(button('実行').disabled).toBe(true); expect(mocks.activate).toHaveBeenCalledTimes(1); expect(mocks.openSession).toHaveBeenCalledTimes(2); expect(button('選択 input')).toBeTruthy();
  });
  it('keeps malformed settings and reserved-looking IDs editable', async () => {
    const malformed = structuredClone(sample); (malformed.nodes[0].config as any).packet.root = { type: 'text', text: { invalid: true } };
    malformed.nodes[0].id = '__proto__'; malformed.edges[0].from.node = '__proto__'; mocks.sample.mockResolvedValue(malformed); await boot();
    expect(button('選択 __proto__')).toBeTruthy(); expect((screen.getByLabelText('設定 JSON') as HTMLTextAreaElement).value).toContain('invalid');
  });
  it('does not let a previous page session keep the reconnected UI busy or overwrite its state', async () => {
    await boot(); await activate();
    const oldReply = reply({ ...report, planVersion: 1 });
    let finish!: (value: SessionResult<Report>) => void;
    mocks.runActive.mockReturnValueOnce(new Promise<SessionResult<Report>>(resolve => { finish = resolve; }));
    fireEvent.click(button('実行'));
    fireEvent(window, new Event('pagehide'));
    const restored = new Event('pageshow'); Object.defineProperty(restored, 'persisted', { value: true }); fireEvent(window, restored);
    fireEvent.click(button('再接続')); await waitFor(() => expect(button('計画を準備').disabled).toBe(false));
    await act(async () => finish(oldReply));
    expect(button('実行').disabled).toBe(true); expect(button('計画を準備').disabled).toBe(false);
    expect(screen.queryByTestId('output-value')).toBeNull(); expect(screen.queryByText('操作中に接続が変更されました。')).toBeNull();
  });
});

it('opens and closes trusted input/execution information without running or changing the graph', async () => {
  await boot();
  fireEvent.click(button('入力と実行の扱い'));
  expect(screen.getByRole('dialog', { name: '入力と実行の扱い' })).toBeTruthy();
  expect(screen.getByText('このEditorでは未対応です。')).toBeTruthy();
  expect(mocks.runActive).not.toHaveBeenCalled();
  expect(mocks.prepare).not.toHaveBeenCalled();
  fireEvent.click(button('閉じる'));
  expect(screen.queryByRole('dialog', { name: '入力と実行の扱い' })).toBeNull();
});
it('keeps fixed graph text in graph storage and explains it separately from runtime persistence',async()=>{
  const graph:Graph={version:1,nodes:[{id:'fixed',kind:'e2e.input',version:1,config:{mode:'fixed',text:'before'}}],edges:[]};
  localStorage.setItem('diy-ime2.editor.v1',JSON.stringify({graph,name:'audit-fixed.json'}));
  await boot();expect((screen.getByLabelText('実行時入力をこのブラウザーに保存') as HTMLInputElement).checked).toBe(false);
  fireEvent.click(button('選択 fixed'));
  fireEvent.change(screen.getByLabelText('設定 JSON'),{target:{value:JSON.stringify({mode:'fixed',text:'fixed-setting-canary'})}});fireEvent.click(button('設定を適用'));
  await waitFor(()=>expect(localStorage.getItem('diy-ime2.editor.v1')).toContain('fixed-setting-canary'));
  expect(localStorage.getItem('diy-ime2.editor.runtime-input.v1')).toBeNull();
  fireEvent.click(button('入力と実行の扱い'));
  expect(screen.getByText('グラフの固定文字列・設定は下書きとして自動保存されます。実行時入力の保存設定とは別です。')).toBeTruthy();
});

it('loads unknown installed Node definitions in their own mode without invoking legacy execution', async () => {
  await boot();const id = 'a'.repeat(32), status = { phase: 'empty', protected: false, sessionEpoch: null, state: null, reviewId: null, physical: { physicalJobs: 0 }, workerProcesses: 0 };
  const externalDefinition = { kind: 'author.custom-node', version: 1, inputs: [], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' }], configSchema: {}, defaultConfig: {} };
  const fetcher = vi.fn(async (path: string, init: RequestInit) => {
    const body = init.body ? JSON.parse(String(init.body)) : {};
    const data = path.endsWith('/connections') ? { connectionId: id, status } : body.method === 'definitions' ? { definitions: [externalDefinition], state: status } : body.method === 'saved' ? { ids: [] } : { closed: true };
    return new Response(JSON.stringify({ ok: true, data }), { status: 200 });
  });vi.stubGlobal('fetch', fetcher);
  fireEvent.click(button('導入済みノードで実行'));fireEvent.click(button('実行Hostに接続'));
  const add = await screen.findByRole('button', { name: 'author.custom-nodeを追加' });fireEvent.click(add);
  expect(screen.getByRole('button', { name: '選択 custom-node_1' })).toBeTruthy();expect(mocks.prepare).not.toHaveBeenCalled();expect(mocks.runActive).not.toHaveBeenCalled();
  fireEvent.click(button('導入済みノードを閉じる'));await waitFor(() => expect(fetcher.mock.calls.some(([, init]) => init.method === 'DELETE')).toBe(true));
  expect(await screen.findByRole('button', { name: '実行' })).toBeTruthy();
});

it('distribution starts externally without opening or falling back to a legacy session', async () => {
  const fetcher = vi.fn(async () => { throw Error('Host unavailable'); });vi.stubGlobal('fetch', fetcher);
  render(<App externalOnly />);
  await waitFor(() => expect(mocks.sample).toHaveBeenCalledWith('official-dictionary'));
  expect(mocks.openSession).not.toHaveBeenCalled();expect(mocks.definitions).not.toHaveBeenCalled();
  expect(screen.queryByRole('button', { name: '非同期実行' })).toBeNull();expect(screen.queryByRole('button', { name: '同期実行に戻る' })).toBeNull();expect(screen.queryByRole('button', { name: '導入済みノードを閉じる' })).toBeNull();
  expect(fetcher).not.toHaveBeenCalled();fireEvent.click(button('実行Hostに接続'));
  await waitFor(() => expect(fetcher).toHaveBeenCalled());
  expect(mocks.openSession).not.toHaveBeenCalled();expect(mocks.definitions).not.toHaveBeenCalled();expect(mocks.runActive).not.toHaveBeenCalled();
});

it('disconnects the external Host when the same sample is opened as another document', async () => {
  const status = { phase: 'empty', protected: false, sessionEpoch: null, state: null, reviewId: null, physical: { physicalJobs: 0 }, workerProcesses: 0 };
  const fetcher = vi.fn(async (path: string, init: RequestInit) => {
    const body = init.body ? JSON.parse(String(init.body)) : {};
    const data = path.endsWith('/connections') ? { connectionId: 'a'.repeat(32), status }
      : body.method === 'definitions' ? { definitions: [], state: status }
      : body.method === 'saved' ? { ids: [] } : { closed: true };
    return new Response(JSON.stringify({ ok: true, data }), { status: 200 });
  });
  vi.stubGlobal('fetch', fetcher);
  render(<App externalOnly />);
  await waitFor(() => expect((screen.getByLabelText('ファイル名') as HTMLInputElement).value).toBe('official-dictionary.json'));
  fireEvent.click(button('実行Hostに接続'));
  await waitFor(() => expect((button('グラフを確認') as HTMLButtonElement).disabled).toBe(false));
  fireEvent.change(screen.getByLabelText('サンプルを読み込む'), { target: { value: 'official-dictionary' } });
  await waitFor(() => expect(fetcher.mock.calls.some(([, init]) => init.method === 'DELETE')).toBe(true));
  expect(await screen.findByRole('button', { name: '実行Hostに接続' })).toBeTruthy();
});
