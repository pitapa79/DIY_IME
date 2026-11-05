import { expect, it } from 'vitest';
import { decimal, runKey, runSnapshot, wireState, type ExecutionRunKey, type ExecutionSnapshot, type ExecutionWireState } from '../src/executionModel';
import { acceptExecution, beginExecution, createExecutionView, executionDataVisible, executionIntent, executionPhysicalStopped,
  loseExecution, observeExecution, recoverExecution, updateExecutionContext, type ExecutionContext } from '../src/executionView';
const epoch = '0123456789abcdef0123456789abcdef';
const input = { revision: 8, text: 'かな😀', mode: 'compose' as const };
function context(fixed = false): ExecutionContext {
  return { sessionEpoch: epoch, state: { revision: '3', activePlanVersion: '3', preparedToken: null, canRollback: true, latestInputRevision: 7, deliveryGeneration: '4' }, input: fixed ? null : { ...input } };
}
function key(): ExecutionRunKey { return { sessionEpoch: epoch, planVersion: '3', runId: '18446744073709551615', inputRevision: 8, deliveryGeneration: '5' }; }
function snapshot(k = key()): ExecutionSnapshot {
  return { key: k, state: 'terminal', eventSeq: '9007199254740993', elapsedMs: 20, current: true,
    physical: { running: 0, cancelRequested: 0, stalled: 0, queued: 0 }, pollAfterMs: 0,
    result: { reason: 'completed', delivery: 'complete', targets: [], ports: [], nodes: [], causes: [], recoveries: [] } };
}
function accepted(fixed = false) {
  const c = context(fixed), intent = executionIntent(c);
  let view = beginExecution(createExecutionView(c), intent);
  const k = key(); if (fixed) { delete k.inputRevision; k.deliveryGeneration = '4'; }
  const state = { ...c.state, latestInputRevision: 8, deliveryGeneration: '5' };
  view = updateExecutionContext(view, { ...c, state });
  view = acceptExecution(view, intent, { state: 'accepted', replayed: false, key: k });
  expect(view.phase).toBe('accepted');
  return { view, intent, key: k };
}
it('validates every v3 identifier without rounding u64 or accepting v2 state', () => {
  for (const value of ['1', '9007199254740993', '18446744073709551615']) expect(decimal(value)).toBe(true);
  for (const value of [1, '01', '', '-1', '1.0', '18446744073709551616', '0']) expect(decimal(value)).toBe(false);
  expect(decimal('0', true)).toBe(true);expect(wireState(context().state)).toBe(true);
  const base: Record<string, unknown> = context().state as unknown as Record<string, unknown>;
  for (const field of Object.keys(base)) { const v = { ...base }; delete v[field]; expect(wireState(v)).toBe(false); }
  expect(wireState({ ...base, revision: 3 })).toBe(false);expect(wireState({ ...base, extra: true })).toBe(false);
  expect(wireState({ revision: 3, active: { version: 3 }, prepared: null, canRollback: true })).toBe(false);
  expect(runKey(key())).toBe(true);expect(runKey({ ...key(), inputRevision: null })).toBe(false);
  expect(runKey({ ...key(), runId: 9007199254740992 })).toBe(false);
});
it('rejects malformed snapshot states, unsafe clocks/counters and inconsistent terminal payloads', () => {
  expect(runSnapshot(snapshot())).toBe(true);
  for (const bad of [{ state: ['terminal'] }, { elapsedMs: Number.MAX_SAFE_INTEGER + 1 }, { eventSeq: '0' }, { physical: { running: -1, cancelRequested: 0, stalled: 0, queued: 0 } }, { result: undefined }]) {
    expect(runSnapshot({ ...snapshot(), ...bad })).toBe(false);
  }
  expect(runSnapshot({ ...snapshot(), result: { ...snapshot().result, delivery: ['complete'] } })).toBe(false);
  expect(runSnapshot({ ...snapshot(), result: { ...snapshot().result, error: { code: 'bad', message: 'bad' } } })).toBe(false);
  expect(runSnapshot({ ...snapshot(), result: { ...snapshot().result, delivery: 'recovered' } })).toBe(false);
  const running = { ...snapshot(), state: 'running' };expect(runSnapshot(running)).toBe(false);
  delete (running as Partial<ExecutionSnapshot>).result;expect(runSnapshot(running)).toBe(true);
});
it('accepts the first start changing the Core input generation, then binds its full real key', () => {
  const { view, intent, key: k } = accepted();
  const terminal = observeExecution(view, intent, snapshot(k));
  expect(executionDataVisible(terminal)).toBe(true);expect(executionPhysicalStopped(terminal)).toBe(true);
  expect(terminal.snapshot!.key.runId).toBe('18446744073709551615');
  expect(acceptExecution(terminal, intent, { state: 'accepted', replayed: true, key: k })).toBe(terminal);
});
it('invalidates immediately on local text, mode or revision edits before any Core reply', () => {
  const { view, intent, key: k } = accepted();const done = observeExecution(view, intent, snapshot(k));
  for (const edit of [{ text: '別入力' }, { mode: 'finalize' as const }, { revision: 9 }]) {
    const changed = updateExecutionContext(done, { ...done.context, input: { ...input, ...edit } });
    expect(changed.phase).toBe('idle');expect(executionDataVisible(changed)).toBe(false);
    expect(observeExecution(changed, intent, snapshot(k))).toBe(changed);
    expect(acceptExecution(changed, intent, { state: 'accepted', replayed: false, key: k })).toBe(changed);
  }
});
it('keeps prepare-only changes and fixed runs across input changes but rejects plan/epoch changes', () => {
  const { view, intent, key: k } = accepted(true);const done = observeExecution(view, intent, snapshot(k));
  for (const c of [{ ...done.context, input: { ...input, revision: 9 } },
    { ...done.context, state: { ...done.context.state, revision: '4', preparedToken: 'new-token' } },
    { ...done.context, state: { ...done.context.state, latestInputRevision: 9, deliveryGeneration: '6' } }]) {
    expect(executionDataVisible(updateExecutionContext(done, c))).toBe(true);
  }
  for (const c of [{ ...done.context, sessionEpoch: 'fedcba9876543210fedcba9876543210' },
    { ...done.context, state: { ...done.context.state, revision: '5', activePlanVersion: '4', deliveryGeneration: '7' } }]) {
    const changed = updateExecutionContext(done, c);expect(changed.phase).toBe('idle');expect(observeExecution(changed, intent, snapshot(k))).toBe(changed);
  }
});
it('rejects every different key field, stale event sequences, old local tokens and non-current data', () => {
  const { view, intent, key: k } = accepted();const done = observeExecution(view, intent, snapshot(k));
  for (const edit of [{ sessionEpoch: 'fedcba9876543210fedcba9876543210' }, { planVersion: '4' }, { runId: '1' }, { inputRevision: 9 }, { deliveryGeneration: '6' }]) {
    expect(observeExecution(done, intent, snapshot({ ...k, ...edit }))).toBe(done);
  }
  expect(observeExecution(done, intent, { ...snapshot(k), eventSeq: '9007199254740992' })).toBe(done);
  const newest = updateExecutionContext(done, { ...done.context, state: { ...done.context.state, revision: '9007199254740993', preparedToken: 'new' } });
  expect(updateExecutionContext(newest, { ...newest.context, state: { ...newest.context.state, revision: '9007199254740992', preparedToken: 'old' } })).toBe(newest);
  const nextIntent = executionIntent(done.context);const next = beginExecution(done, nextIntent);
  expect(observeExecution(next, intent, snapshot(k))).toBe(next);
  expect(acceptExecution(next, intent, { state: 'accepted', replayed: false, key: k })).toBe(next);
  const stale = observeExecution(done, intent, { ...snapshot(k), current: false });expect(executionDataVisible(stale)).toBe(false);
  expect(executionDataVisible(observeExecution(stale, intent, snapshot(k)))).toBe(false);
});
it('preserves terminal logical results while true physical counts change, never reviving late data', () => {
  const { view, intent, key: k } = accepted();const cancelled: ExecutionSnapshot = { ...snapshot(k), physical: { running: 0, cancelRequested: 1, stalled: 0, queued: 0 }, result: { ...snapshot(k).result!, reason: 'cancelled', delivery: 'cancelled' } };
  const done = observeExecution(view, intent, cancelled);expect(executionDataVisible(done)).toBe(false);expect(executionPhysicalStopped(done)).toBe(false);
  const late = observeExecution(done, intent, { ...snapshot(k), eventSeq: '9007199254740994', elapsedMs: 999 });
  expect(late.snapshot!.result).toBe(done.snapshot!.result);expect(late.snapshot!.elapsedMs).toBe(20);expect(executionDataVisible(late)).toBe(false);expect(executionPhysicalStopped(late)).toBe(true);
  const running = { ...snapshot(k), state: 'running' as const, eventSeq: '9007199254740995' };delete running.result;
  expect(observeExecution(late, intent, running)).toBe(late);
  const expired = observeExecution(view, intent, { ...cancelled, result: { ...cancelled.result!, reason: 'expired', delivery: 'expired' } });expect(executionDataVisible(expired)).toBe(false);
});
it('shows recovered completion without hiding diagnostic payloads and keeps faults/retention unknown', () => {
  const { view, intent, key: k } = accepted();const recovery = { ...snapshot(k), result: { ...snapshot(k).result!, delivery: 'recovered' as const, causes: [{ original: 'failure' }], recoveries: [{ nodeId: 'recover' }] } };
  const done = observeExecution(view, intent, recovery);expect(executionDataVisible(done)).toBe(true);expect(done.snapshot!.result!.causes).toEqual(recovery.result.causes);
  for (const forgotten of [false, true]) { const lost = loseExecution(done, intent, forgotten);expect(lost.phase).toBe(forgotten ? 'forgotten' : 'unknown');expect(executionPhysicalStopped(lost)).toBeUndefined();expect(executionDataVisible(lost)).toBe(false);expect(observeExecution(lost, intent, snapshot(k))).toBe(lost); }
});

it('recovers unknown acceptance only through an explicit same-epoch lookup, never from retention loss', () => {
  const { view, intent, key: k } = accepted();const done = observeExecution(view, intent, snapshot(k));const lost = loseExecution(done, intent);
  const recovered = recoverExecution(lost, intent, k);expect(recovered.phase).toBe('accepted');expect(executionDataVisible(recovered)).toBe(false);
  expect(executionDataVisible(observeExecution(recovered, intent, snapshot(k)))).toBe(true);
  expect(recoverExecution(lost, intent, { ...k, runId: '1' })).toBe(lost);
  const forgotten = loseExecution(done, intent, true);expect(recoverExecution(forgotten, intent, k)).toBe(forgotten);
  const changed = updateExecutionContext(lost, { ...lost.context, sessionEpoch: 'fedcba9876543210fedcba9876543210' });expect(recoverExecution(changed, intent, k)).toBe(changed);
});

it('does not call a zero-count accepted run physically stopped before it becomes terminal', () => {
  const { view, intent, key: k } = accepted();const pending = { ...snapshot(k), state: 'accepted' as const };delete pending.result;
  const waiting = observeExecution(view, intent, pending);expect(executionPhysicalStopped(waiting)).toBeUndefined();
  const running = observeExecution(waiting, intent, { ...pending, state: 'running', eventSeq: '9007199254740994', physical: { running: 1, cancelRequested: 0, stalled: 0, queued: 0 } });expect(executionPhysicalStopped(running)).toBe(false);
});
