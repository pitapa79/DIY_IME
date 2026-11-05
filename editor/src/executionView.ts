import { runtimeInputKey, type RuntimeInput } from './model';
import { sameRun, type ExecutionReceipt, type ExecutionRunKey, type ExecutionSnapshot, type ExecutionWireState } from './executionModel';

export interface ExecutionContext {
  sessionEpoch: string;
  state: ExecutionWireState;
  input: RuntimeInput | null;
}
/** Object identity is the local request token: the same input can run twice. */
export interface ExecutionIntent {
  readonly sessionEpoch: string;
  readonly planVersion: string;
  readonly deliveryGeneration: string;
  readonly input: Readonly<RuntimeInput> | null;
}
export interface ExecutionView {
  context: ExecutionContext;
  phase: 'idle' | 'starting' | 'accepted' | 'running' | 'terminal' | 'unknown' | 'forgotten';
  intent?: ExecutionIntent;
  key?: ExecutionRunKey;
  snapshot?: ExecutionSnapshot;
}
const inputKey = (input: Readonly<RuntimeInput> | null) => input === null ? null : runtimeInputKey(input);
export function createExecutionView(context: ExecutionContext): ExecutionView { return { context, phase: 'idle' }; }
export function executionIntent(context: ExecutionContext): ExecutionIntent {
  if (context.state.activePlanVersion === null) throw new Error('稼働中の計画が必要です。');
  return Object.freeze({ sessionEpoch: context.sessionEpoch, planVersion: context.state.activePlanVersion,
    deliveryGeneration: context.state.deliveryGeneration,
    input: context.input === null ? null : Object.freeze({ ...context.input }) });
}
function matchesContext(intent: ExecutionIntent, context: ExecutionContext, key?: ExecutionRunKey) {
  return intent.sessionEpoch === context.sessionEpoch && intent.planVersion === context.state.activePlanVersion
    && (intent.input === null || inputKey(intent.input) === inputKey(context.input)
      && (key === undefined || context.state.latestInputRevision === key.inputRevision
        && context.state.deliveryGeneration === key.deliveryGeneration));
}
function matchesKey(intent: ExecutionIntent, key: ExecutionRunKey) {
  return intent.sessionEpoch === key.sessionEpoch && intent.planVersion === key.planVersion
    && intent.input?.revision === key.inputRevision;
}
/** Local edits invalidate before Core acknowledges them. prepare-only changes
 * keep the current run. Fixed runs ignore input edits, never plan/epoch changes. */
export function updateExecutionContext(view: ExecutionView, context: ExecutionContext): ExecutionView {
  if (context.sessionEpoch === view.context.sessionEpoch
    && (BigInt(context.state.revision) < BigInt(view.context.state.revision)
      || BigInt(context.state.deliveryGeneration) < BigInt(view.context.state.deliveryGeneration))) return view;
  return view.intent && matchesContext(view.intent, context, view.key) ? { ...view, context } : createExecutionView(context);
}
export function beginExecution(view: ExecutionView, intent: ExecutionIntent): ExecutionView {
  return matchesContext(intent, view.context) ? { context: view.context, phase: 'starting', intent } : view;
}
export function acceptExecution(view: ExecutionView, intent: ExecutionIntent, receipt: ExecutionReceipt): ExecutionView {
  if (view.intent !== intent || !matchesContext(intent, view.context, receipt.key) || !matchesKey(intent, receipt.key)
    || view.key && !sameRun(view.key, receipt.key)) return view;
  // A late accepted receipt must not replace a newer running/terminal snapshot.
  if (view.snapshot || view.phase === 'unknown' || view.phase === 'forgotten') return view;
  return { ...view, phase: 'accepted', key: receipt.key };
}
/** Shared ordering rule for displayed and background-tracked runs. A late
 * poll cannot revive a terminal phase while its physical cleanup is observed. */
export function advanceExecutionSnapshot(previous: ExecutionSnapshot | undefined, snapshot: ExecutionSnapshot): ExecutionSnapshot | undefined {
  if (previous && (!sameRun(previous.key, snapshot.key) || BigInt(snapshot.eventSeq) < BigInt(previous.eventSeq)
    || ['accepted', 'running', 'terminal'].indexOf(snapshot.state) < ['accepted', 'running', 'terminal'].indexOf(previous.state))) return;
  const next = previous?.state === 'terminal'
    ? { ...snapshot, state: previous.state, result: previous.result, elapsedMs: previous.elapsedMs }
    : snapshot;
  return { ...next, current: previous?.current === false ? false : next.current };
}
export function observeExecution(view: ExecutionView, intent: ExecutionIntent, snapshot: ExecutionSnapshot): ExecutionView {
  if (view.intent !== intent || !view.key || !matchesContext(intent, view.context, view.key)
    || !matchesKey(intent, snapshot.key) || !sameRun(view.key, snapshot.key)
    || view.phase === 'unknown' || view.phase === 'forgotten') return view;
  const next = advanceExecutionSnapshot(view.snapshot, snapshot);
  return next ? { ...view, phase: next.state, snapshot: next } : view;
}
export function loseExecution(view: ExecutionView, intent: ExecutionIntent, forgotten = false): ExecutionView {
  if (view.intent !== intent) return view;
  // Drop displayable data, but keep identity. No claim of non-execution or stop.
  return { context: view.context, intent, key: view.key, phase: forgotten ? 'forgotten' : 'unknown' };
}
/** Explicit get-start recovery only. This never starts or resends work and
 * cannot restore a different epoch/key or a definitively forgotten result. */
export function recoverExecution(view: ExecutionView, intent: ExecutionIntent, key: ExecutionRunKey): ExecutionView {
  if (view.phase !== 'unknown' || view.intent !== intent || !matchesContext(intent, view.context, key)
    || !matchesKey(intent, key) || view.key && !sameRun(view.key, key)) return view;
  return { context: view.context, intent, key, phase: 'accepted' };
}
export function executionDataVisible(view: ExecutionView) {
  const snapshot = view.snapshot;
  return !!(view.intent && view.key && snapshot && matchesContext(view.intent, view.context, view.key)
    && sameRun(view.key, snapshot.key) && snapshot.current && snapshot.state === 'terminal'
    && snapshot.result?.reason === 'completed' && ['complete', 'recovered'].includes(snapshot.result.delivery));
}
export function executionPhysicalStopped(view: ExecutionView): boolean | undefined {
  const physical = view.snapshot?.physical;
  if (!physical) return undefined;
  if (Object.values(physical).some(count => count > 0)) return false;
  return view.snapshot!.state === 'terminal' ? true : undefined;
}
