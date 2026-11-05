// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import ExecutionPanel from '../src/ExecutionPanel';
import { createExecutionApi, ExecutionApiError } from '../src/executionApi';
import type { ExecutionClient } from '../src/useExecutionSession';
import type { ExecutionRunKey, ExecutionSnapshot, ExecutionStart, ExecutionWireState } from '../src/executionModel';
import type { Graph, RuntimeInput } from '../src/model';
const epoch = '0123456789abcdef0123456789abcdef', nextEpoch = 'fedcba9876543210fedcba9876543210';
const graph: Graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }], edges: [] };
const definition = { kind: 'input.text', version: 1, inputs: [], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'any' as const }], configSchema: {}, defaultConfig: {} };
const initial: RuntimeInput = { revision: 1, text: '最初の入力', mode: 'compose' };
function fixture() {
  let state: ExecutionWireState = { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' };
  let runId = 0, event = 1, currentEpoch = epoch;
  const snapshots = new Map<string, ExecutionSnapshot>(), inputs = new Map<string, RuntimeInput | undefined>();
  const reply = <T,>(result: T) => ({ state: structuredClone(state), result });
  const finish = (id = String(runId), reason: 'completed' | 'cancelled' | 'expired' = 'completed', physical = 0) => {
    const before = snapshots.get(id)!;const input = inputs.get(id);
    const snap: ExecutionSnapshot = { ...before, state: 'terminal', eventSeq: String(++event), current: input === undefined || input.revision === state.latestInputRevision, elapsedMs: 20,
      physical: { running: 0, cancelRequested: physical, stalled: 0, queued: 0 },
      result: { reason, delivery: reason === 'completed' ? 'complete' : reason, targets: [{ node: 'source', port: 'out' }], ports: reason === 'completed' ? [{ endpoint: { node: 'source', port: 'out' }, value: { status: 'data', packet: { version: 1, root: { type: 'text', text: input?.text ?? '固定' } } } }] : [], nodes: [{ nodeId: 'source', status: reason === 'completed' ? 'executed' : 'discarded', diagnostic: null }], causes: [], recoveries: [] } };
    snapshots.set(id, snap);return snap;
  };
  const open = vi.fn().mockImplementation(async () => { if (open.mock.calls.length > 1) currentEpoch = nextEpoch;state = { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' };return { id: currentEpoch, state: structuredClone(state), resourceDomain: 'shared', resourcePolicy: {} }; });
  const close = vi.fn().mockResolvedValue({ closed: true });
  const definitions = vi.fn().mockImplementation(async () => reply([{ version: 2, executionKind: 'sync', definition, errorInputs: [] }]));
  const prepare = vi.fn().mockImplementation(async () => { state = { ...state, revision: String(BigInt(state.revision) + 1n), preparedToken: 'token' };return reply({ token: 'token' }); });
  const activate = vi.fn().mockImplementation(async () => { state = { ...state, revision: String(BigInt(state.revision) + 1n), activePlanVersion: String(BigInt(state.activePlanVersion ?? '0') + 1n), canRollback: state.activePlanVersion !== null, preparedToken: null, deliveryGeneration: String(BigInt(state.deliveryGeneration) + 1n) };return reply({}); });
  const rollback = vi.fn().mockImplementation(async () => { state = { ...state, activePlanVersion: String(BigInt(state.activePlanVersion!) + 1n), revision: String(BigInt(state.revision) + 1n), deliveryGeneration: String(BigInt(state.deliveryGeneration) + 1n), canRollback: false, preparedToken: null };return reply({}); });
  const invalidate = vi.fn().mockImplementation(async (_id, _plan, revision: number) => { state = { ...state, latestInputRevision: revision, deliveryGeneration: String(BigInt(state.deliveryGeneration) + 1n) };return reply({}); });
  const start = vi.fn().mockImplementation(async (_id, payload: ExecutionStart) => {
    if (payload.input && state.latestInputRevision !== payload.input.revision) state = { ...state, latestInputRevision: payload.input.revision, deliveryGeneration: String(BigInt(state.deliveryGeneration) + 1n) };
    const key: ExecutionRunKey = { sessionEpoch: currentEpoch, planVersion: state.activePlanVersion!, runId: String(++runId), ...(payload.input ? { inputRevision: payload.input.revision } : {}), deliveryGeneration: state.deliveryGeneration };
    inputs.set(key.runId, payload.input);snapshots.set(key.runId, { key, state: 'running', current: true, eventSeq: String(++event), elapsedMs: 1, physical: { running: 1, cancelRequested: 0, stalled: 0, queued: 0 }, pollAfterMs: 1 });return reply({ state: 'accepted', replayed: false, key });
  });
  const getRun = vi.fn().mockImplementation(async (_id, key: ExecutionRunKey) => reply(structuredClone(snapshots.get(key.runId)!)));
  const cancel = vi.fn().mockImplementation(async (_id, key: ExecutionRunKey) => reply(finish(key.runId, 'cancelled', 1)));
  const client: ExecutionClient = { ...createExecutionApi(vi.fn()), open, close, definitions, prepare, activate, rollback, invalidate, start, getRun, cancel };
  const props = { graph, input: initial, hasDrafts: false, composing: false, onDefinitions: vi.fn(), onCatalogError: vi.fn(), onStatuses: vi.fn(), onClearConnection: vi.fn(), onRestartInput: vi.fn(), client };
  return { props, client, open, close, definitions, prepare, activate, rollback, invalidate, start, getRun, cancel, snapshots, finish, reply };
}
const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement;
async function ready(f: ReturnType<typeof fixture>) {
  const ui = render(<ExecutionPanel {...f.props} />);await waitFor(() => expect(button('計画を準備').disabled).toBe(false));
  fireEvent.click(button('計画を準備'));await waitFor(() => expect(button('切替').disabled).toBe(false));fireEvent.click(button('切替'));await waitFor(() => expect(button('実行').disabled).toBe(false));return ui;
}
afterEach(() => { cleanup();vi.useRealTimers(); });
it('opens v3 declarations, sends explicit targets without a default deadline, and displays a keyed result and cable', async () => {
  const f = fixture();const ui = await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));
  expect(f.start.mock.calls[0][1]).toMatchObject({ startKey: '1', expectedPlanVersion: '1', targets: [{ node: 'source', port: 'out' }], input: initial });expect(f.start.mock.calls[0][1]).not.toHaveProperty('runBudgetMs');
  expect(screen.queryByTestId('v3-output-value')).toBeNull();act(() => { f.finish(); });expect((await screen.findByTestId('v3-output-value')).textContent).toBe(initial.text);
  ui.rerender(<ExecutionPanel {...f.props} connection={{ from: { node: 'source', port: 'out' }, to: { node: 'next', port: 'in' } }} />);expect(screen.getByText('接続データ')).toBeTruthy();expect(screen.getByTestId('v3-output-value').textContent).toBe(initial.text);
  ui.rerender(<ExecutionPanel {...f.props} hasDrafts />);expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(f.props.onStatuses).toHaveBeenLastCalledWith(null);expect(f.definitions).toHaveBeenCalledTimes(1);
});
it('hides old input immediately without resubmitting', async () => {
  const f = fixture();const ui = await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));act(() => { f.finish(); });await screen.findByTestId('v3-output-value');
  const changed = { ...initial, revision: 2, text: '新しい入力', mode: 'finalize' as const };ui.rerender(<ExecutionPanel {...f.props} input={changed} />);expect(screen.queryByTestId('v3-output-value')).toBeNull();await waitFor(() => expect(f.invalidate).toHaveBeenCalled());expect(f.start).toHaveBeenCalledTimes(1);
  expect(screen.getByText(/以前の実行の結果は非表示/)).toBeTruthy();
});
it('keeps cancellation distinct from real physical ending and does not expose late Data', async () => {
  const f = fixture();await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(button('取消').disabled).toBe(false));
  fireEvent.click(button('取消'));await waitFor(() => expect(f.cancel).toHaveBeenCalledTimes(1));await screen.findByText(/確定状態：結果の採用取消/);
  expect(f.props.onStatuses).toHaveBeenLastCalledWith({ source: 'discarded' });
  expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(screen.getByText(/取消待ち 1/)).toBeTruthy();expect(screen.getByText(/実際の処理終了は未確認/)).toBeTruthy();
  act(() => { const old = f.snapshots.get('1')!;f.snapshots.set('1', { ...old, eventSeq: '99', physical: { running: 0, cancelRequested: 0, stalled: 0, queued: 0 } }); });
  await screen.findByText(/実際の処理終了を確認しました/);expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(f.start).toHaveBeenCalledTimes(1);
});
it('does not restore a plan or rerun after a fault, and reconnects only explicitly', async () => {
  const f = fixture();await ready(f);f.getRun.mockRejectedValueOnce(new ExecutionApiError('connection-lost', '接続不明', true));fireEvent.click(button('実行'));
  await screen.findByText('接続不明');expect(f.close).toHaveBeenCalledTimes(1);expect(f.open).toHaveBeenCalledTimes(1);expect(screen.queryByTestId('v3-output-value')).toBeNull();
  fireEvent.click(button('再接続'));await waitFor(() => expect(f.open).toHaveBeenCalledTimes(2));await waitFor(() => expect(button('計画を準備').disabled).toBe(false));expect(button('実行').disabled).toBe(true);expect(f.prepare).toHaveBeenCalledTimes(1);expect(f.start).toHaveBeenCalledTimes(1);
});
it('waits for the latest one input in auto mode and never starts the intermediate input', async () => {
  const f = fixture();const ui = await ready(f);vi.useFakeTimers();fireEvent.click(screen.getByRole('checkbox', { name: '非同期の入力を自動実行' }));
  await act(async () => { await vi.advanceTimersByTimeAsync(200); });expect(f.start).toHaveBeenCalledTimes(1);
  ui.rerender(<ExecutionPanel {...f.props} input={{ ...initial, revision: 2, text: '中間' }} />);await act(async () => {});
  ui.rerender(<ExecutionPanel {...f.props} input={{ ...initial, revision: 3, text: '最新' }} />);await act(async () => { await vi.advanceTimersByTimeAsync(250); });expect(f.start).toHaveBeenCalledTimes(1);
  act(() => { f.finish('1', 'cancelled'); });await act(async () => { await vi.advanceTimersByTimeAsync(100); });await act(async () => { await vi.advanceTimersByTimeAsync(0); });expect(f.start).toHaveBeenCalledTimes(2);expect(f.start.mock.calls[1][1].input.text).toBe('最新');expect(f.start.mock.calls.some(call => call[1].input.text === '中間')).toBe(false);vi.useRealTimers();ui.unmount();
});

it('distinguishes retained-result loss from an unexecuted start and does not retry it', async () => {
  const f = fixture();await ready(f);f.getRun.mockRejectedValueOnce(new ExecutionApiError('result-gone', '保持期間が終了しました', false));fireEvent.click(button('実行'));
  await screen.findByText('保持期間が終了しました');expect(screen.getByRole('status').textContent).toBe('結果の保持切れ');expect(f.start).toHaveBeenCalledTimes(1);expect(f.open).toHaveBeenCalledTimes(1);expect(screen.queryByTestId('v3-output-value')).toBeNull();
});
it('does not revive the tracked running phase when an old poll arrives after cancellation', async () => {
  const f = fixture();await ready(f);
  let finishPoll!: (value: ReturnType<typeof f.reply>) => void;
  const delayed = new Promise<ReturnType<typeof f.reply>>(resolve => { finishPoll = resolve; });f.getRun.mockReturnValueOnce(delayed);
  fireEvent.click(button('実行'));await waitFor(() => expect(f.getRun).toHaveBeenCalledTimes(1));
  const old = f.reply(structuredClone(f.snapshots.get('1')!));
  fireEvent.click(button('取消'));await screen.findByText(/確定状態：結果の採用取消/);expect(button('実行').disabled).toBe(false);
  await act(async () => { finishPoll(old); });
  expect(button('実行').disabled).toBe(false);expect(button('取消').disabled).toBe(true);expect(screen.queryByTestId('v3-output-value')).toBeNull();
});

it('keeps prepare inert, invalidates activation and never resurrects an old result on rollback', async () => {
  const f = fixture();const ui = await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));act(() => { f.finish(); });await screen.findByTestId('v3-output-value');
  fireEvent.click(button('計画を準備'));await waitFor(() => expect(button('切替').disabled).toBe(false));expect(screen.getByTestId('v3-output-value').textContent).toBe(initial.text);
  const next: Graph = { ...graph, nodes: [...graph.nodes, { id: 'second', kind: 'input.text', version: 1 }] };ui.rerender(<ExecutionPanel {...f.props} graph={next} />);expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(button('切替').disabled).toBe(true);
  fireEvent.click(button('計画を準備'));await waitFor(() => expect(button('切替').disabled).toBe(false));fireEvent.click(button('切替'));await screen.findByText('稼働中 v2');expect(screen.queryByTestId('v3-output-value')).toBeNull();
  fireEvent.click(button('復帰'));await screen.findByText('稼働中 v3');ui.rerender(<ExecutionPanel {...f.props} />);expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(button('復帰').disabled).toBe(true);expect(f.start).toHaveBeenCalledTimes(1);
});
it('keeps a fixed-input run visible across unrelated runtime input edits', async () => {
  const f = fixture();f.props.graph = { version: 1, nodes: [{ id: 'source', kind: 'demo.packet', version: 1 }], edges: [] };
  f.definitions.mockImplementation(async () => f.reply([{ version: 2, executionKind: 'sync', definition: { ...definition, kind: 'demo.packet' }, errorInputs: [] }]));
  const ui = await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));expect(f.start.mock.calls[0][1]).not.toHaveProperty('input');act(() => { f.finish(); });await screen.findByTestId('v3-output-value');
  ui.rerender(<ExecutionPanel {...f.props} input={{ ...initial, text: '無関係な入力', revision: 2 }} />);expect(screen.getByTestId('v3-output-value').textContent).toBe('固定');expect(f.invalidate).not.toHaveBeenCalled();
});
it('rejects an old completed poll after a local edit and its Core invalidation acknowledgement', async () => {
  const f = fixture();const ui = await ready(f);let resolve!: (value: ReturnType<typeof f.reply>) => void;f.getRun.mockReturnValueOnce(new Promise<ReturnType<typeof f.reply>>(done => { resolve = done; }));
  fireEvent.click(button('実行'));await waitFor(() => expect(f.getRun).toHaveBeenCalledTimes(1));const old = f.reply(f.finish());
  ui.rerender(<ExecutionPanel {...f.props} input={{ ...initial, text: '今の入力', revision: 2 }} />);await waitFor(() => expect(f.invalidate).toHaveBeenCalled());await act(async () => resolve(old));
  expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(f.start).toHaveBeenCalledTimes(1);
});
it('sends only an explicit valid deadline and keeps expiry/stalled ending separate from late Data', async () => {
  const f = fixture();await ready(f);fireEvent.change(screen.getByRole('textbox', { name: '非同期の実行期限' }), { target: { value: '0' } });fireEvent.click(button('実行'));await screen.findByText('実行期限は正の整数ms、または空欄にしてください。');expect(f.start).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('textbox', { name: '非同期の実行期限' }), { target: { value: '500' } });fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));expect(f.start.mock.calls[0][1].runBudgetMs).toBe(500);
  act(() => { const expired = f.finish('1', 'expired');f.snapshots.set('1', { ...expired, physical: { running: 0, cancelRequested: 0, stalled: 1, queued: 0 } }); });await screen.findByText(/確定状態：期限切れ/);expect(screen.getByText(/停止未確認 1/)).toBeTruthy();expect(screen.queryByTestId('v3-output-value')).toBeNull();
  act(() => { f.finish(); });await screen.findByText(/実際の処理終了を確認しました/);expect(screen.getByText(/確定状態：期限切れ/)).toBeTruthy();expect(screen.queryByTestId('v3-output-value')).toBeNull();
});
it('does not retry an automatically started input after a known admission refusal', async () => {
  const f = fixture();const ui = await ready(f);f.start.mockRejectedValueOnce(new ExecutionApiError('busy', '実行枠不足', false, f.reply({}).state));vi.useFakeTimers();fireEvent.click(screen.getByRole('checkbox', { name: '非同期の入力を自動実行' }));
  await act(async () => { await vi.advanceTimersByTimeAsync(200); });expect(f.start).toHaveBeenCalledTimes(1);await act(async () => { await vi.advanceTimersByTimeAsync(5000); });expect(f.start).toHaveBeenCalledTimes(1);expect(screen.getByText('実行枠不足')).toBeTruthy();vi.useRealTimers();ui.unmount();
});
it('shows a recovered output alongside the original failure and keeps error cables free of Data', async () => {
  const f = fixture();const inputPort = (id: string) => ({ port: { ...definition.outputs[0], id }, required: true });
  const failure = { ...definition, kind: 'test.failure', inputs: [inputPort('in')] }, recovery = { ...definition, kind: 'test.recovery', inputs: [inputPort('err')] };
  f.props.graph = { version: 1, nodes: [...graph.nodes, { id: 'upstream', kind: failure.kind, version: 1 }, { id: 'recover', kind: recovery.kind, version: 1 }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'upstream', port: 'in' } }, { from: { node: 'upstream', port: 'out' }, to: { node: 'recover', port: 'err' } }] };
  f.definitions.mockImplementation(async () => f.reply([definition, failure, recovery].map(def => ({ version: 2, executionKind: 'sync', definition: def, errorInputs: def.kind === recovery.kind ? ['err'] : [] }))));
  const ui = await ready(f);fireEvent.click(button('実行'));await waitFor(() => expect(f.start).toHaveBeenCalledTimes(1));
  act(() => {
    const original = f.finish(), key = original.key, id = { sessionEpoch: key.sessionEpoch, planVersion: key.planVersion, runId: key.runId, nodeId: 'upstream' };
    const diagnostic = { ...id, port: 'out', error: { code: 'upstream-failure', message: '元の失敗' } };
    f.snapshots.set('1', { ...original, result: { reason: 'completed', delivery: 'recovered', targets: [{ node: 'recover', port: 'out' }],
      ports: [{ endpoint: { node: 'upstream', port: 'out' }, value: { status: 'error', causes: [id] } }, { endpoint: { node: 'recover', port: 'out' }, value: { status: 'data', packet: { version: 1, root: { type: 'text', text: '復旧した文章' } } } }],
      nodes: [{ nodeId: 'upstream', status: 'failed', diagnostic }, { nodeId: 'recover', status: 'executed', diagnostic: null }], causes: [{ id, diagnostic, paths: [] }], recoveries: [{ nodeId: 'recover', input: 'err', output: 'out', causes: [id] }] } });
  });
  expect((await screen.findByTestId('v3-output-value')).textContent).toBe('復旧した文章');expect(screen.getByText(/upstream-failure — 元の失敗/)).toBeTruthy();expect(screen.getByText(/復旧：recover err → out/)).toBeTruthy();
  ui.rerender(<ExecutionPanel {...f.props} connection={f.props.graph.edges[1]} />);expect(screen.queryByTestId('v3-output-value')).toBeNull();expect(screen.getByText('この出力はエラーです。原因を確認してください。')).toBeTruthy();expect(screen.getByText(/upstream-failure — 元の失敗/)).toBeTruthy();
});
