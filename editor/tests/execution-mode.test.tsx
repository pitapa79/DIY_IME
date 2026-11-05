// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import App from '../src/App';
import { ExecutionApiError } from '../src/executionApi';
const legacy = vi.hoisted(() => ({ definitions: vi.fn(), sample: vi.fn(), openSession: vi.fn(), closeSession: vi.fn(), sessionStatus: vi.fn(), sessionValidate: vi.fn(), prepare: vi.fn(), activate: vi.fn(), runActive: vi.fn(), rollback: vi.fn() }));
const v3 = vi.hoisted(() => ({ open: vi.fn(), close: vi.fn(), definitions: vi.fn(), status: vi.fn() }));
vi.mock('../src/api', async original => ({ ...await original<typeof import('../src/api')>(), api: legacy }));
vi.mock('../src/executionApi', async original => ({ ...await original<typeof import('../src/executionApi')>(), createExecutionApi: () => v3 }));
vi.mock('@xyflow/react', () => ({ useReactFlow: () => ({ fitView: vi.fn(), fitBounds: vi.fn(), getNodesBounds: vi.fn() }), useNodesInitialized: () => false,
  ReactFlow: () => <div aria-label="canvas" />, Background: () => null, Controls: () => null, Handle: () => null, BackgroundVariant: { Dots: 'dots' }, MarkerType: { ArrowClosed: 'arrowclosed' }, Position: { Left: 'left', Right: 'right' } }));
const graph = { version: 1, nodes: [{ id: 'source', kind: 'input.text', version: 1 }], edges: [] };
const node = { kind: 'input.text', version: 1, inputs: [], outputs: [{ id: 'out', dataType: { id: 'text-packet', version: 1 }, language: null, root: 'any' }], configSchema: {}, defaultConfig: {} };
beforeEach(() => {
  localStorage.clear();vi.resetAllMocks();vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { cb(0);return 1; });
  legacy.definitions.mockResolvedValue([node]);legacy.sample.mockResolvedValue(graph);legacy.closeSession.mockResolvedValue(undefined);
  legacy.openSession.mockResolvedValue({ id: 'old', state: { revision: 0, active: null, prepared: null, canRollback: false } });
  v3.open.mockResolvedValue({ id: '0123456789abcdef0123456789abcdef', resourceDomain: 'shared', resourcePolicy: {}, state: { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' } });
  v3.close.mockResolvedValue({ closed: true });v3.definitions.mockResolvedValue({ state: { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' }, result: [{ version: 2, executionKind: 'sync', definition: node, errorInputs: [] }] });
});
afterEach(() => { cleanup();vi.unstubAllGlobals(); });
it('keeps v2 by default and selects v3 explicitly without changing the graph or mixing old controls', async () => {
  render(<App />);await waitFor(() => expect((screen.getByRole('button', { name: '非同期実行' }) as HTMLButtonElement).disabled).toBe(false));expect(v3.open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '非同期実行' }));await waitFor(() => expect(v3.definitions).toHaveBeenCalledTimes(1));expect(v3.open).toHaveBeenCalledTimes(1);
  expect(screen.getAllByRole('button', { name: '実行' })).toHaveLength(1);expect(screen.getAllByRole('button', { name: '計画を準備' })).toHaveLength(1);expect(screen.queryByRole('checkbox', { name: '入力を自動実行' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'グラフ JSON' }));expect(JSON.parse((screen.getByRole('textbox', { name: 'グラフ JSON' }) as HTMLTextAreaElement).value)).toEqual(graph);expect(v3.close).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '同期実行に戻る' }));await waitFor(() => expect(legacy.definitions).toHaveBeenCalledTimes(2));expect(v3.close).toHaveBeenCalledTimes(1);expect(legacy.runActive).not.toHaveBeenCalled();
  expect(JSON.parse((screen.getByRole('textbox', { name: 'グラフ JSON' }) as HTMLTextAreaElement).value)).toEqual(graph);
});
it('does not fall back to v2 execution when v3 is unconfigured', async () => {
  v3.open.mockRejectedValueOnce(new ExecutionApiError('host-unconfigured', 'v3の資源設定が必要です', true));render(<App />);
  await waitFor(() => expect((screen.getByRole('button', { name: '非同期実行' }) as HTMLButtonElement).disabled).toBe(false));fireEvent.click(screen.getByRole('button', { name: '非同期実行' }));await screen.findByText('v3の資源設定が必要です');
  expect(v3.open).toHaveBeenCalledTimes(1);expect(legacy.runActive).not.toHaveBeenCalled();expect((screen.getByRole('button', { name: '実行' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => {});expect(v3.open).toHaveBeenCalledTimes(1);
});

it('waits for the v3 declaration catalogue before rebuilding the canvas handles and edges', async () => {
  let resolve!: (value: unknown) => void;v3.definitions.mockReturnValueOnce(new Promise(done => { resolve = done; }));render(<App />);
  await waitFor(() => expect((screen.getByRole('button', { name: '非同期実行' }) as HTMLButtonElement).disabled).toBe(false));expect(screen.getByLabelText('canvas')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: '非同期実行' }));await waitFor(() => expect(v3.definitions).toHaveBeenCalledTimes(1));expect(screen.queryByLabelText('canvas')).toBeNull();
  await act(async () => resolve({ state: { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' }, result: [{ version: 2, executionKind: 'sync', definition: node, errorInputs: [] }] }));
  await screen.findByLabelText('canvas');expect(legacy.runActive).not.toHaveBeenCalled();
});
