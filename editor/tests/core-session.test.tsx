// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useCoreSession } from '../src/useCoreSession';
import type { CoreSession } from '../src/model';

const mocks = vi.hoisted(() => ({ openSession: vi.fn(), closeSession: vi.fn(), sessionStatus: vi.fn() }));
vi.mock('../src/api', async importOriginal => ({ ...await importOriginal<typeof import('../src/api')>(), api: mocks }));
const empty = { revision: 0, active: null, prepared: null, canRollback: false };
beforeEach(() => { vi.resetAllMocks(); mocks.openSession.mockResolvedValue({ id: 'initial', state: empty }); mocks.closeSession.mockResolvedValue(undefined); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

it('does not run connection recovery callbacks after unmounting', async () => {
  const hook = renderHook(useCoreSession); await waitFor(() => expect(hook.result.current.session?.id).toBe('initial'));
  let finish!: (value: CoreSession) => void;
  mocks.openSession.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  const recovered = vi.fn(); let pending!: Promise<void>;
  act(() => { pending = hook.result.current.connect(recovered); });
  await waitFor(() => expect(mocks.openSession).toHaveBeenCalledTimes(2));
  hook.unmount();
  await act(async () => { finish({ id: 'late', state: empty }); await pending; });
  expect(recovered).not.toHaveBeenCalled(); expect(mocks.closeSession).toHaveBeenCalledWith('late');
});

it('ignores a status response from the old connection after recovery', async () => {
  vi.useFakeTimers();
  const hook = renderHook(useCoreSession); await act(async () => {});
  expect(hook.result.current.session?.id).toBe('initial');
  let finish!: (value: { state: typeof empty }) => void;
  mocks.sessionStatus.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  await act(async () => { vi.advanceTimersByTime(30_000); });
  expect(mocks.sessionStatus).toHaveBeenCalledWith('initial');
  mocks.openSession.mockResolvedValueOnce({ id: 'new', state: empty });
  const recovered = vi.fn(); await act(async () => { await hook.result.current.connect(recovered); });
  await act(async () => { finish({ state: { ...empty, revision: 999 } }); });
  expect(recovered).toHaveBeenCalledTimes(1); expect(hook.result.current.session).toEqual({ id: 'new', state: empty });
});
