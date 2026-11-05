// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useExecutionSession, type ExecutionClient } from '../src/useExecutionSession';
import { createExecutionApi, ExecutionApiError } from '../src/executionApi';
import type { ExecutionSession, ExecutionWireState } from '../src/executionModel';
const epoch = '0123456789abcdef0123456789abcdef', nextEpoch = 'fedcba9876543210fedcba9876543210';
const state: ExecutionWireState = { revision: '0', activePlanVersion: null, preparedToken: null, canRollback: false, latestInputRevision: null, deliveryGeneration: '1' };
const session = (id = epoch, update = {}): ExecutionSession => ({ id, state: { ...state, ...update }, resourceDomain: 'shared', resourcePolicy: {} });
function deferred<T>() { let resolve!: (value: T) => void, reject!: (reason: unknown) => void;const promise = new Promise<T>((yes, no) => { resolve = yes;reject = no; });return { promise, resolve, reject }; }
function fixture() {
  const open = vi.fn().mockResolvedValue(session()), close = vi.fn().mockResolvedValue({ closed: true }), status = vi.fn().mockResolvedValue({ state, result: {} });
  const client: ExecutionClient = { ...createExecutionApi(vi.fn()), open, close, status };
  return { client, open, close, status };
}
afterEach(() => { cleanup();vi.useRealTimers(); });
it('does not open v3 while disabled and starts an empty Session only on explicit enable', async () => {
  const f = fixture();const hook = renderHook(({ enabled }) => useExecutionSession(enabled, f.client), { initialProps: { enabled: false } });
  await act(async () => {});expect(f.open).not.toHaveBeenCalled();expect(hook.result.current.connection).toBe('disconnected');
  hook.rerender({ enabled: true });await waitFor(() => expect(hook.result.current.session?.id).toBe(epoch));expect(f.open).toHaveBeenCalledTimes(1);expect(hook.result.current.session!.state.activePlanVersion).toBeNull();
  hook.rerender({ enabled: false });expect(f.close).toHaveBeenCalledWith(epoch);expect(hook.result.current.session).toBeNull();expect(f.open).toHaveBeenCalledTimes(1);
});
it('closes a late open after unmount and never adopts it', async () => {
  const f = fixture(), pending = deferred<ExecutionSession>();f.open.mockReturnValue(pending.promise);
  const hook = renderHook(() => useExecutionSession(true, f.client));hook.unmount();
  await act(async () => pending.resolve(session()));expect(f.close).toHaveBeenCalledTimes(1);expect(f.close).toHaveBeenCalledWith(epoch);
});
it('keeps the newest explicit connection when opens finish in reverse order', async () => {
  const f = fixture(), first = deferred<ExecutionSession>(), second = deferred<ExecutionSession>();f.open.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  const hook = renderHook(() => useExecutionSession(true, f.client));let connecting!: Promise<void>;
  act(() => { connecting = hook.result.current.connect(); });await act(async () => { second.resolve(session(nextEpoch));await connecting; });
  await act(async () => first.resolve(session()));expect(hook.result.current.session!.id).toBe(nextEpoch);expect(f.close).toHaveBeenCalledWith(epoch);expect(f.close).not.toHaveBeenCalledWith(nextEpoch);
});
it('does not let StrictMode cleanup close the new owner when the old open arrives late', async () => {
  const f = fixture(), first = deferred<ExecutionSession>();f.open.mockReturnValueOnce(first.promise).mockResolvedValueOnce(session(nextEpoch));
  const hook = renderHook(() => useExecutionSession(true, f.client), { reactStrictMode: true });
  expect(f.open).toHaveBeenCalledTimes(2);await waitFor(() => expect(hook.result.current.session?.id).toBe(nextEpoch));await act(async () => first.resolve(session()));expect(f.close).toHaveBeenCalledWith(epoch);expect(f.close).not.toHaveBeenCalledWith(nextEpoch);
});
it('does not serialize cancel and polling behind a pending plan mutation', async () => {
  const f = fixture();const hook = renderHook(() => useExecutionSession(true, f.client));await waitFor(() => expect(hook.result.current.session).not.toBeNull());
  const pending = deferred<{ state: ExecutionWireState; result: {} }>();let prepare!: Promise<unknown>;
  act(() => { prepare = hook.result.current.call('prepare', () => pending.promise); });expect(hook.result.current.busy).toBe('prepare');
  await expect(hook.result.current.call('activate', async () => ({ state, result: {} }))).rejects.toMatchObject({ code: 'busy', resultUnknown: false });
  const reading = deferred<{ state: ExecutionWireState; result: {} }>(), cancelling = deferred<{ state: ExecutionWireState; result: {} }>();let poll!: Promise<unknown>, cancel!: Promise<unknown>;
  act(() => { poll = hook.result.current.call('get-run', () => reading.promise);cancel = hook.result.current.call('cancel', () => cancelling.promise); });
  expect(hook.result.current.polling).toBe(true);expect(hook.result.current.cancelling).toBe(true);
  await expect(hook.result.current.call('get-run', async () => ({ state, result: {} }))).rejects.toMatchObject({ code: 'busy', resultUnknown: false });
  await expect(hook.result.current.call('cancel', async () => ({ state, result: {} }))).rejects.toMatchObject({ code: 'busy', resultUnknown: false });
  await act(async () => { cancelling.resolve({ state, result: {} });await cancel;reading.resolve({ state, result: {} });await poll; });
  expect(hook.result.current.busy).toBe('prepare');await act(async () => { pending.resolve({ state, result: {} });await prepare; });expect(hook.result.current.busy).toBeNull();
});
it('rejects old responses after reconnect and preserves BigInt state order across independent lanes', async () => {
  const f = fixture();const hook = renderHook(() => useExecutionSession(true, f.client));await waitFor(() => expect(hook.result.current.session).not.toBeNull());
  const delayed = deferred<{ state: ExecutionWireState; result: {} }>();let read!: Promise<unknown>;
  act(() => { read = hook.result.current.call('get-run', () => delayed.promise); });
  const newest = { ...state, revision: '9007199254740993', deliveryGeneration: '9007199254740993' };
  await act(async () => { await hook.result.current.call('activate', async () => ({ state: newest, result: {} })); });
  await act(async () => { delayed.resolve({ state: { ...newest, revision: '9007199254740992' }, result: {} });await read; });expect(hook.result.current.session!.state).toEqual(newest);
  const old = deferred<{ state: ExecutionWireState; result: {} }>();let stale!: Promise<unknown>;
  act(() => { stale = hook.result.current.call('get-run', () => old.promise).catch(error => error); });
  f.open.mockResolvedValueOnce(session(nextEpoch));await act(async () => { await hook.result.current.connect(); });
  await act(async () => { old.resolve({ state: newest, result: {} });expect(await stale).toMatchObject({ code: 'session-lost', resultUnknown: true }); });expect(hook.result.current.session!.id).toBe(nextEpoch);expect(hook.result.current.session!.state).toEqual(state);
});
it('keeps known Core refusal state but invalidates only this Session on an unknown failure', async () => {
  const f = fixture();const hook = renderHook(() => useExecutionSession(true, f.client));await waitFor(() => expect(hook.result.current.session).not.toBeNull());
  const changed = { ...state, revision: '2', preparedToken: 'actual' };
  await act(async () => { await hook.result.current.call('start', async () => { throw new ExecutionApiError('busy', 'full', false, changed); }).catch(() => {}); });
  expect(hook.result.current.connection).toBe('connected');expect(hook.result.current.session!.state).toEqual(changed);expect(f.close).not.toHaveBeenCalled();
  await act(async () => { await hook.result.current.call('start', async () => { throw new ExecutionApiError('connection-lost', 'unknown', true); }).catch(() => {}); });
  expect(hook.result.current.connection).toBe('lost');expect(hook.result.current.session).toBeNull();expect(f.close).toHaveBeenCalledTimes(1);expect(f.open).toHaveBeenCalledTimes(1);expect(hook.result.current.busy).toBeNull();
});
it('pagehide/pageshow close once, do not reconnect automatically and dispose an in-flight request', async () => {
  const f = fixture();const hook = renderHook(() => useExecutionSession(true, f.client));await waitFor(() => expect(hook.result.current.session).not.toBeNull());
  const pending = deferred<{ state: ExecutionWireState; result: {} }>();let read!: Promise<unknown>;
  act(() => { read = hook.result.current.call('get-run', () => pending.promise).catch(error => error);window.dispatchEvent(new Event('pagehide'));window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })); });
  await act(async () => { pending.resolve({ state, result: {} });expect(await read).toMatchObject({ resultUnknown: true }); });
  expect(hook.result.current.session).toBeNull();expect(f.close).toHaveBeenCalledTimes(1);expect(f.open).toHaveBeenCalledTimes(1);hook.unmount();expect(f.close).toHaveBeenCalledTimes(1);
});
it('performs read-only heartbeat when idle, skips a mutation and stops monitoring after failure', async () => {
  vi.useFakeTimers();const f = fixture();const hook = renderHook(() => useExecutionSession(true, f.client));await act(async () => {});
  const pending = deferred<{ state: ExecutionWireState; result: {} }>();let changing!: Promise<unknown>;
  act(() => { changing = hook.result.current.call('prepare', () => pending.promise); });await act(async () => vi.advanceTimersByTime(30_000));expect(f.status).not.toHaveBeenCalled();
  await act(async () => { pending.resolve({ state, result: {} });await changing; });await act(async () => vi.advanceTimersByTime(30_000));expect(f.status).toHaveBeenCalledTimes(1);
  f.status.mockRejectedValueOnce(new ExecutionApiError('connection-lost', 'unknown', true));await act(async () => vi.advanceTimersByTime(30_000));expect(hook.result.current.connection).toBe('lost');
  await act(async () => vi.advanceTimersByTime(60_000));expect(f.status).toHaveBeenCalledTimes(2);expect(f.open).toHaveBeenCalledTimes(1);
});
