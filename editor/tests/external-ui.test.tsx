// @vitest-environment jsdom
import { emptyTrace } from '../src/executionTrace';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import ExternalExecutionPanel from '../src/ExternalExecutionPanel';
import { ExternalApiError, type ExternalClient, type ExternalStatus } from '../src/externalApi';
import type { Definition, Graph, RuntimeInput } from '../src/model';
import type { ExecutionSnapshot } from '../src/executionModel';
const epoch = 'b'.repeat(32), id = 'a'.repeat(32), reviewId = 'c'.repeat(32);
const graph: Graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }, { id: 'worker', kind: 'fixture.wasm', version: 1 }], edges: [{ from: { node: 'source', port: 'out' }, to: { node: 'worker', port: 'in' } }] };
const input: RuntimeInput = { revision: 1, text: 'current-input', mode: 'compose' };
const port = { id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'text' as const };
const definitions: Definition[] = graph.nodes.map(n => ({ kind: n.kind, version: 1, inputs: [], outputs: [port], configSchema: {}, defaultConfig: {} }));
const requests = [{ id: 'd'.repeat(64), kind: 'execute' as const, packageId: 'test.external', nodes: ['worker'], source: null }, { id: 'e'.repeat(64), kind: 'inputFlow' as const, packageId: 'test.external', nodes: ['worker'], source: { node: 'source', port: 'out' } }];
function fixture(saved = false) {
  let status: ExternalStatus = { phase: 'empty', protected: false, sessionEpoch: null, state: null, reviewId: null, physical: { physicalJobs: 0 }, workerProcesses: 0 };
  const key = { sessionEpoch: epoch, planVersion: '1', runId: '1', inputRevision: 1, deliveryGeneration: '1' };
  const snapshot: ExecutionSnapshot = { key, state: 'terminal', eventSeq: '3', elapsedMs: 5, current: true, physical: { running: 0, cancelRequested: 0, stalled: 0, queued: 0 }, pollAfterMs: 50,
    result: { reason: 'completed', delivery: 'complete', targets: [{ node: 'worker', port: 'out' }], ports: [{ endpoint: { node: 'worker', port: 'out' }, value: { status: 'data', packet: { version: 1, root: { type: 'text', text: 'private-result' } } } }], nodes: [], causes: [], recoveries: [] } };
  const client = {
    traceStart: vi.fn(async () => emptyTrace()), traceStartDetailed: vi.fn(async () => emptyTrace()), traceRead: vi.fn(async () => emptyTrace()), traceStop: vi.fn(async () => emptyTrace()), traceClear: vi.fn(async () => emptyTrace()),
    secrets: vi.fn(async () => ({ available: true, entries: [] as { id: string; version: string }[] })),
    secretPut: vi.fn(async (_id: string, name: string, _value: string) => ({ entry: { id: name, version: 'f'.repeat(32) }, state: { ...status, phase: 'empty' as const, sessionEpoch: null, state: null } })),
    secretDelete: vi.fn(async () => ({ ...status, phase: 'empty' as const, sessionEpoch: null, state: null })),
    open: vi.fn(async () => ({ connectionId: id, status })), close: vi.fn(async () => {}),
    status: vi.fn(async () => status), saved: vi.fn(async () => saved ? [requests[0].id] : []), definitions: vi.fn(async () => ({ definitions, state: status })),
    prepare: vi.fn(async () => { status = { ...status, phase: 'review', sessionEpoch: epoch, reviewId };return { reviewId, sessionEpoch: epoch, requests, missing: saved ? [] : requests }; }),
    approve: vi.fn(async () => []), activate: vi.fn(async () => { status = { ...status, phase: 'ready', state: { revision: '0', activePlanVersion: '1', preparedToken: null, canRollback: false, latestInputRevision: 1, deliveryGeneration: '1' } };return status; }),
    start: vi.fn(async () => ({ key, state: status })), getRun: vi.fn(async () => ({ snapshot, state: status })), cancelRun: vi.fn(async () => ({ snapshot, state: status })), getStart: vi.fn(async () => ({ key, state: status })),
    invalidate: vi.fn(async (_id: string, _version: string, revision: number) => { status = { ...status, state: { ...status.state!, latestInputRevision: revision, deliveryGeneration: '2' } };return status; }),
    protect: vi.fn(async (_id: string, enabled: boolean) => { status = { ...status, protected: enabled, phase: enabled ? 'protected' : 'empty', sessionEpoch: null, state: null }; }),
    revoke: vi.fn(async () => { status = { ...status, phase: 'empty', state: null, sessionEpoch: null }; }), cancel: vi.fn(async () => {}),
  };
  const props = { graph, input, composing: false, hasDrafts: false, onClearConnection: vi.fn(), onDefinitions: vi.fn(), onCatalogError: vi.fn(), onStatuses: vi.fn(), client: client as ExternalClient };
  return { client, props, snapshot };
}
const click = (name: string) => fireEvent.click(screen.getByRole('button', { name }));
async function connectAndPrepare() { click('実行Hostに接続');await waitFor(() => expect((screen.getByRole('button', { name: 'グラフを確認' }) as HTMLButtonElement).disabled).toBe(false));click('グラフを確認');await screen.findByRole('region', { name: '実行の承認' }); }
async function activate(saved = false) { await connectAndPrepare();if (!saved) { for (const box of Array.from(screen.getByRole('region', { name: '実行の承認' }).querySelectorAll('input[type=checkbox]'))) fireEvent.click(box);click('選択した操作を今回だけ許可');await waitFor(() => expect((screen.getByRole('button', { name: 'このグラフへ切替' }) as HTMLButtonElement).disabled).toBe(false)); }click('このグラフへ切替');await waitFor(() => expect((screen.getByRole('button', { name: '承認済みグラフを実行' }) as HTMLButtonElement).disabled).toBe(false)); }
afterEach(cleanup);
it('requires explicit per-operation consent and shows the source port and destination', async () => {
  const { client, props } = fixture();render(<ExternalExecutionPanel {...props} />);await connectAndPrepare();
  expect(screen.getByText('source.out → workerへの入力を許可')).toBeTruthy();expect(client.start).not.toHaveBeenCalled();expect(client.approve).not.toHaveBeenCalled();
  for (const box of Array.from(screen.getByRole('region', { name: '実行の承認' }).querySelectorAll('input[type=checkbox]'))) fireEvent.click(box);click('選択した操作の許可を保存');
  await waitFor(() => expect(client.approve).toHaveBeenCalledWith(id, expect.objectContaining({ reviewId }), requests.map(r => r.id), true));
  expect(client.activate).not.toHaveBeenCalled();
});
it('reuses saved subjects without another approval and hides delivered data immediately on input/graph edits', async () => {
  const { client, props } = fixture(true);const view = render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByText('private-result');
  expect(client.approve).not.toHaveBeenCalled();view.rerender(<ExternalExecutionPanel {...props} input={{ ...input, revision: 2, text: 'changed' }} />);
  expect(screen.queryByText('private-result')).toBeNull();await waitFor(() => expect(client.invalidate).toHaveBeenCalled());expect(client.prepare).toHaveBeenCalledTimes(1);
  view.rerender(<ExternalExecutionPanel {...props} graph={{ ...graph, nodes: [...graph.nodes, { id: 'extra', kind: 'input.text', version: 1 }] }} />);expect(screen.queryByText('private-result')).toBeNull();
});
it('withdraws a result and awaits old Host exit when opening a document with the same graph and input', async () => {
  const { client, props } = fixture(true);
  let finishClose!: () => void;
  client.close.mockImplementationOnce(() => new Promise<void>(resolve => { finishClose = resolve; }));
  const view = render(<ExternalExecutionPanel {...props} documentScope={1} />);
  await activate(true);
  click('承認済みグラフを実行');
  await screen.findByText('private-result');

  view.rerender(<ExternalExecutionPanel {...props} documentScope={2} />);
  expect(screen.queryByText('private-result')).toBeNull();
  expect(screen.getByRole('status').textContent).toBe('終了確認中');
  expect(screen.getByRole<HTMLButtonElement>('button', { name: '実行Hostに接続' }).disabled).toBe(true);
  expect(client.close).toHaveBeenCalledWith(id);
  await act(async () => finishClose());
  expect(screen.getByRole<HTMLButtonElement>('button', { name: '実行Hostに接続' }).disabled).toBe(false);
});
it('queries a lost start response across an input edit without resending', async () => {
  const { client, props } = fixture(true);client.start.mockRejectedValueOnce(new ExternalApiError('external-connection', 'lost'));
  const view = render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByRole('button', { name: '開始結果を照会' });
  view.rerender(<ExternalExecutionPanel {...props} input={{ ...input, revision: 2, text: 'changed' }} />);
  await waitFor(() => expect((screen.getByRole('button', { name: '開始結果を照会' }) as HTMLButtonElement).disabled).toBe(false));click('開始結果を照会');
  await waitFor(() => expect(client.getStart).toHaveBeenCalledWith(id, '1'));expect(client.start).toHaveBeenCalledTimes(1);expect(screen.queryByText('private-result')).toBeNull();
});
it('withdraws already delivered data before protection and never restores it on unprotect', async () => {
  const { client, props } = fixture(true);render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByText('private-result');
  click('入力を保護');expect(screen.queryByText('private-result')).toBeNull();await screen.findByRole('button', { name: '保護を解除' });
  await waitFor(() => expect((screen.getByRole('button', { name: '保護を解除' }) as HTMLButtonElement).disabled).toBe(false));click('保護を解除');
  await waitFor(() => expect(client.protect).toHaveBeenCalledWith(id, false));expect(screen.queryByText('private-result')).toBeNull();expect(client.activate).toHaveBeenCalledTimes(1);
});
it('keeps disconnect pending until true Host exit and scopes unmount cleanup to its old lease', async () => {
  const { client, props } = fixture(true);let end!: () => void;client.close.mockImplementation(() => new Promise<void>(resolve => { end = resolve; }));
  const view = render(<ExternalExecutionPanel {...props} />);await activate(true);click('切断');expect(screen.getByText('終了確認中')).toBeTruthy();expect((screen.getByRole('button', { name: '実行Hostに接続' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => end());view.unmount();expect(client.close).toHaveBeenCalledTimes(1);expect(client.close).toHaveBeenCalledWith(id);
});
it('rejects a late payload after a protection action even while the original query remains pending', async () => {
  const { client, props, snapshot } = fixture(true);let finish!: (value: any) => void;client.getRun.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await waitFor(() => expect(client.getRun).toHaveBeenCalled());
  click('入力を保護');await waitFor(() => expect(client.protect).toHaveBeenCalledWith(id, true));
  const oldState: ExternalStatus = { phase: 'ready', protected: false, sessionEpoch: epoch, reviewId: null, state: { revision: '0', activePlanVersion: '1', preparedToken: null, canRollback: false, latestInputRevision: 1, deliveryGeneration: '1' }, physical: { physicalJobs: 0 }, workerProcesses: 0 };
  await act(async () => finish({ snapshot, state: oldState }));expect(screen.queryByText('private-result')).toBeNull();expect(screen.getByRole('button', { name: '保護を解除' })).toBeTruthy();
});
it('can withdraw a Session-only grant and clears results before the acknowledgment', async () => {
  const { client, props } = fixture();render(<ExternalExecutionPanel {...props} />);await activate();click('承認済みグラフを実行');await screen.findByText('private-result');
  fireEvent.click(screen.getAllByRole('button', { name: 'この操作の許可を撤回' })[0]);expect(screen.queryByText('private-result')).toBeNull();await waitFor(() => expect(client.revoke).toHaveBeenCalledWith(id, requests[0].id));expect(client.approve).toHaveBeenCalledWith(id, expect.anything(), expect.anything(), false);
});
it('disconnects and hides delivered results on pagehide without silently reconnecting on pageshow', async () => {
  const { client, props } = fixture(true);render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByText('private-result');
  fireEvent(window, new Event('pagehide'));expect(screen.queryByText('private-result')).toBeNull();await waitFor(() => expect(client.close).toHaveBeenCalledWith(id));fireEvent(window, new Event('pageshow'));expect(client.open).toHaveBeenCalledTimes(1);
});
it('hides delivered data when Host status cannot be confirmed and preserves a queryable start key', async () => {
  const { client, props } = fixture(true);render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByText('private-result');
  client.status.mockRejectedValueOnce(new ExternalApiError('external-connection', 'status lost'));
  await screen.findByText('status lost', {}, { timeout: 3000 });expect(screen.queryByText('private-result')).toBeNull();expect(screen.getByRole('button', { name: '開始結果を照会' })).toBeTruthy();expect(client.start).toHaveBeenCalledTimes(1);
});

it('closes a connection that finishes opening after pagehide and holds the UI until its true ending', async () => {
  const { client, props } = fixture(true);let open!: (v: any) => void, end!: () => void;
  client.open.mockImplementation(() => new Promise(resolve => { open = resolve; }));client.close.mockImplementation(() => new Promise<void>(resolve => { end = resolve; }));
  render(<ExternalExecutionPanel {...props} />);click('実行Hostに接続');fireEvent(window, new Event('pagehide'));
  await act(async () => open({ connectionId: id, status: { phase: 'empty', protected: false, sessionEpoch: null, state: null, reviewId: null, physical: { physicalJobs: 0 }, workerProcesses: 0 } }));
  expect(client.close).toHaveBeenCalledWith(id);expect(client.definitions).not.toHaveBeenCalled();expect((screen.getByRole('button', { name: '実行Hostに接続' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => end());expect((screen.getByRole('button', { name: '実行Hostに接続' }) as HTMLButtonElement).disabled).toBe(false);
});

it('presents dictionary reading as a separate explicit operation with a logical binding and relative reference', async () => {
  const { client, props } = fixture();
  const resource = { id: 'f'.repeat(64), kind: 'resourceRead' as const, packageId: 'test.external', nodes: ['worker'], source: null, resource: { binding: 'dictionary', reference: 'sample.tsv', policySha256: '0'.repeat(64) } };
  vi.spyOn(props.client, 'prepare').mockResolvedValue({ reviewId, sessionEpoch: epoch, requests: [resource], missing: [resource] });
  render(<ExternalExecutionPanel {...props} />);await connectAndPrepare();
  const checkbox = screen.getByRole('checkbox', { name: 'dictionary の sample.tsv を読み込む（worker）' });expect((checkbox as HTMLInputElement).checked).toBe(false);expect(client.approve).not.toHaveBeenCalled();
  fireEvent.click(checkbox);click('選択した操作を今回だけ許可');await waitFor(() => expect(client.approve).toHaveBeenCalledWith(id, expect.objectContaining({ reviewId }), [resource.id], false));
});
it('masks a new secret, submits only on explicit save, clears its input and delivered results before waiting', async () => {
  const { client, props } = fixture(true);render(<ExternalExecutionPanel {...props} />);await activate(true);click('承認済みグラフを実行');await screen.findByText('private-result');
  click('API秘密の一覧を更新');await screen.findByLabelText('API秘密の値');expect(client.secretPut).not.toHaveBeenCalled();
  const field = screen.getByLabelText('API秘密の値') as HTMLInputElement;expect(field.type).toBe('password');
  fireEvent.change(screen.getByLabelText('API秘密の識別名'), { target: { value: 'service' } });fireEvent.change(field, { target: { value: 'private-api-canary' } });
  let finish!: (value: any) => void;client.secretPut.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));click('秘密を保存（同じ識別名は置換）');
  await waitFor(() => expect(client.secretPut).toHaveBeenCalledWith(id, 'service', 'private-api-canary'));expect(field.value).toBe('');expect(screen.queryByText('private-result')).toBeNull();expect(screen.queryByText('private-api-canary')).toBeNull();
  await act(async () => finish({ entry: { id: 'service', version: 'f'.repeat(32) }, state: { phase: 'empty', protected: false, sessionEpoch: null, reviewId: null, state: null, workerProcesses: 0, physical: {} } }));
  expect(client.start).toHaveBeenCalledTimes(1);expect(client.secretPut).toHaveBeenCalledTimes(1);
});
it('never restores a secret input across protection or pagehide and requires a fresh explicit submission after an uncertain save', async () => {
  const { client, props } = fixture();render(<ExternalExecutionPanel {...props} />);await connectAndPrepare();click('API秘密の一覧を更新');await screen.findByLabelText('API秘密の値');
  const field = screen.getByLabelText('API秘密の値') as HTMLInputElement;fireEvent.change(field, { target: { value: 'private-hidden-canary' } });click('入力を保護');await screen.findByRole('button', { name: '保護を解除' });expect(field.value).toBe('');
  fireEvent.change(screen.getByLabelText('API秘密の識別名'), { target: { value: 'service' } });fireEvent.change(field, { target: { value: 'lost-save-canary' } });client.secretPut.mockRejectedValueOnce(new ExternalApiError('external-connection', '保存結果を確認できません。'));
  click('秘密を保存（同じ識別名は置換）');await screen.findByText('保存結果を確認できません。');expect(field.value).toBe('');expect(client.secretPut).toHaveBeenCalledTimes(1);
  fireEvent.change(field, { target: { value: 'leave-canary' } });await act(async () => { window.dispatchEvent(new Event('pagehide')); });expect(screen.queryByDisplayValue('leave-canary')).toBeNull();expect(client.secretPut).toHaveBeenCalledTimes(1);
});
it('shows the fixed network destination and a separate credential-use choice without approving both together', async () => {
  const { client, props } = fixture();
  const network = { binding: 'api', method: 'POST' as const, destination: 'https://service.example/fixed', policySha256: '0'.repeat(64), limits: { maxCalls: 2, maxRequestBytes: 4096, maxResponseBytes: 8192, maxContinuationBytes: 1024, budgetMs: 30000 }, secret: { id: 'service', version: 'f'.repeat(32) } };
  const operations = [{ id: '1'.repeat(64), kind: 'networkUse' as const, packageId: 'test', nodes: ['worker'], source: null, network }, { id: '2'.repeat(64), kind: 'secretUse' as const, packageId: 'test', nodes: ['worker'], source: null, network }];
  vi.spyOn(props.client, 'prepare').mockResolvedValue({ reviewId, sessionEpoch: epoch, requests: operations, missing: operations });
  render(<ExternalExecutionPanel {...props} />);await connectAndPrepare();
  const communication = screen.getByRole('checkbox', { name: /POST https:\/\/service.example\/fixed への通信を許可/ });
  const credential = screen.getByRole('checkbox', { name: /秘密 service.*認証に使用/ });
  expect((communication as HTMLInputElement).checked).toBe(false);expect((credential as HTMLInputElement).checked).toBe(false);
  expect(client.start).not.toHaveBeenCalled();fireEvent.click(communication);click('選択した操作を今回だけ許可');
  await waitFor(() => expect(client.approve).toHaveBeenCalledWith(id, expect.objectContaining({ reviewId }), [operations[0].id], false));
  expect(client.activate).not.toHaveBeenCalled();expect(client.secretPut).not.toHaveBeenCalled();
});

it('hides cancellation only after logical completion and all physical owners have ended', async () => {
  const {client, props}=fixture(true);render(<ExternalExecutionPanel {...props}/>);await activate(true);click('承認済みグラフを実行');
  await screen.findByText('private-result');expect(client.getRun).toHaveBeenCalled();expect(screen.queryByRole('button',{name:'実行を中止'})).toBeNull();
});
it('keeps cancellation available for a logically terminal run with an unconfirmed physical owner', async () => {
  const {client, props, snapshot}=fixture(true);snapshot.physical.stalled=1;delete snapshot.result;
  render(<ExternalExecutionPanel {...props}/>);await activate(true);click('承認済みグラフを実行');
  await waitFor(()=>expect(client.getRun).toHaveBeenCalled());expect(screen.getByRole('button',{name:'実行を中止'})).toBeTruthy();
  snapshot.physical.stalled=0;click('実行を中止');await waitFor(()=>expect(client.cancelRun).toHaveBeenCalled());
  await waitFor(()=>expect(screen.queryByRole('button',{name:'実行を中止'})).toBeNull());
});
