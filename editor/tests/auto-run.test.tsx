// @vitest-environment jsdom
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AUTO_RUN_DELAY_MS, useAutoRun } from '../src/useAutoRun';

beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(0); });
afterEach(() => { cleanup(); vi.useRealTimers(); });
const advance = async (ms = AUTO_RUN_DELAY_MS) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });
function setup() {
  const run = vi.fn().mockResolvedValue(undefined);
  const options = { scope: 'session-a:plan-1' as string | null, inputKey: 'r1:k:compose', blocked: false, run };
  const hook = renderHook(props => useAutoRun(props), { initialProps: options });
  const enable = () => act(() => hook.result.current.setEnabled(true));
  return { ...hook, options, run, enable };
}

it('starts OFF, waits for the debounce, and does not repeat a completed input', async () => {
  const hook = setup(); await advance(1000); expect(hook.run).not.toHaveBeenCalled();
  hook.enable(); await advance(199); expect(hook.run).not.toHaveBeenCalled();
  await advance(1); expect(hook.run).toHaveBeenCalledTimes(1);
  hook.rerender({ ...hook.options, blocked: true });
  hook.rerender(hook.options); await advance(1000); expect(hook.run).toHaveBeenCalledTimes(1);
});

it('debounces rapid changes and invokes only the newest callback and snapshot', async () => {
  const hook = setup(); hook.enable(); await advance(100);
  const latest = vi.fn().mockResolvedValue(undefined);
  hook.rerender({ ...hook.options, inputKey: 'r2:ky:compose' }); await advance(100);
  hook.rerender({ ...hook.options, inputKey: 'r3:kya:compose', run: latest });
  await advance(199); expect(hook.run).not.toHaveBeenCalled(); expect(latest).not.toHaveBeenCalled();
  await advance(1); expect(latest).toHaveBeenCalledTimes(1);
});

it('keeps just the latest input while busy, then runs it when the existing operation finishes', async () => {
  const hook = setup(); hook.enable(); await advance(); expect(hook.run).toHaveBeenCalledTimes(1);
  hook.rerender({ ...hook.options, blocked: true, inputKey: 'r2:ky:compose' }); await advance();
  hook.rerender({ ...hook.options, blocked: true, inputKey: 'r3:kya:compose' }); await advance(1000);
  expect(hook.run).toHaveBeenCalledTimes(1);
  const latest = vi.fn().mockResolvedValue(undefined);
  hook.rerender({ ...hook.options, inputKey: 'r3:kya:compose', run: latest }); await advance(0);
  expect(latest).toHaveBeenCalledTimes(1);
});

it('honors the remaining debounce time after a short busy period', async () => {
  const hook = setup(); hook.rerender({ ...hook.options, blocked: true }); hook.enable(); await advance(100);
  hook.rerender(hook.options); await advance(99); expect(hook.run).not.toHaveBeenCalled();
  await advance(1); expect(hook.run).toHaveBeenCalledTimes(1);
});

it('cancels a pending duplicate when the same snapshot is run manually', async () => {
  const hook = setup(); hook.enable(); await advance(100);
  act(() => hook.result.current.consume()); await advance(1000); expect(hook.run).not.toHaveBeenCalled();
  hook.rerender({ ...hook.options, inputKey: 'r2:k:finalize' }); await advance();
  expect(hook.run).toHaveBeenCalledTimes(1);
});

it('drops pending work when switched OFF and allows a deliberate retry when enabled again', async () => {
  const hook = setup(); hook.enable(); await advance(100);
  act(() => hook.result.current.setEnabled(false)); await advance(1000); expect(hook.run).not.toHaveBeenCalled();
  hook.enable(); await advance(); expect(hook.run).toHaveBeenCalledTimes(1);
});

it('does not send a queued run to an old session or plan', async () => {
  const hook = setup(); hook.enable(); await advance(100);
  hook.rerender({ ...hook.options, scope: null }); await advance(1000); expect(hook.run).not.toHaveBeenCalled();
  const next = vi.fn().mockResolvedValue(undefined);
  hook.rerender({ ...hook.options, scope: 'session-b:plan-2', run: next });
  await advance(199); expect(next).not.toHaveBeenCalled(); await advance(1);
  expect(next).toHaveBeenCalledTimes(1); expect(hook.run).not.toHaveBeenCalled();
});

it('never loops on a rejected run without a new input or explicit retry', async () => {
  const hook = setup(); hook.run.mockRejectedValue(new Error('conversion failed')); hook.enable();
  await advance(10_000); expect(hook.run).toHaveBeenCalledTimes(1);
  hook.rerender({ ...hook.options, inputKey: 'r2:ka:compose' }); await advance(); expect(hook.run).toHaveBeenCalledTimes(2);
});

it('turns OFF on pagehide and clears timers when unmounted', async () => {
  const hook = setup(); hook.enable(); await advance(100);
  act(() => window.dispatchEvent(new Event('pagehide'))); await advance(1000);
  expect(hook.result.current.enabled).toBe(false); expect(hook.run).not.toHaveBeenCalled();
  hook.enable(); hook.unmount(); await advance(1000); expect(hook.run).not.toHaveBeenCalled();
});
