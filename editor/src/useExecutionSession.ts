import { useCallback, useEffect, useRef, useState } from 'react';
import { createExecutionApi, ExecutionApiError } from './executionApi';
import type { ExecutionReply, ExecutionSession, ExecutionWireState } from './executionModel';

export type ExecutionClient = ReturnType<typeof createExecutionApi>;
export type ExecutionOperation = 'definitions' | 'validate' | 'prepare' | 'activate' | 'rollback' | 'start' | 'invalidate' | 'get-start' | 'get-run' | 'cancel' | 'status';
type Lane = 'mutation' | 'poll' | 'cancel';
interface Owner { session: ExecutionSession | null; disposed: boolean; closeSent: boolean }
interface Slot { owner: Owner; operation: ExecutionOperation }
const laneFor = (operation: ExecutionOperation): Lane => operation === 'cancel' ? 'cancel'
  : ['definitions', 'get-start', 'get-run', 'status'].includes(operation) ? 'poll' : 'mutation';

/** Owns only this UI's Session. It never kills/restarts a Host or restores work.
 * Keep an injected client stable for the lifetime of this hook. */
export function useExecutionSession(enabled = false, injected?: ExecutionClient) {
  const defaultClient = useRef<ExecutionClient | null>(null);
  if (!defaultClient.current) defaultClient.current = createExecutionApi();
  const client = injected ?? defaultClient.current;
  const [session, setSession] = useState<ExecutionSession | null>(null);
  const [connection, setConnection] = useState<'disconnected' | 'connecting' | 'connected' | 'lost'>('disconnected');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<ExecutionOperation | null>(null);
  const [polling, setPolling] = useState(false), [cancelling, setCancelling] = useState(false);
  const mounted = useRef(false), owner = useRef<Owner | null>(null);
  const slots = useRef(new Map<Lane, Slot>());
  const publishSlots = useCallback(() => {
    if (!mounted.current) return;
    setBusy(slots.current.get('mutation')?.operation ?? null);
    setPolling(slots.current.has('poll'));setCancelling(slots.current.has('cancel'));
  }, []);
  const close = useCallback((scope: Owner | null) => {
    if (!scope) return;
    scope.disposed = true;
    if (scope.session && !scope.closeSent) {
      scope.closeSent = true;
      // An unconfirmed close is not physical termination. No retry or abort.
      void client.close(scope.session.id).catch(() => {});
    }
  }, [client]);
  const active = useCallback((scope: Owner) => mounted.current && owner.current === scope && !scope.disposed, []);
  const acceptState = useCallback((scope: Owner, state: ExecutionWireState) => {
    if (!active(scope) || !scope.session) return;
    const previous = scope.session.state;
    if (BigInt(state.revision) < BigInt(previous.revision) || BigInt(state.deliveryGeneration) < BigInt(previous.deliveryGeneration)) return;
    scope.session = { ...scope.session, state };setSession(scope.session);
  }, [active]);
  const connect = useCallback(async () => {
    if (!enabled || !mounted.current) return;
    close(owner.current);
    const scope: Owner = { session: null, disposed: false, closeSent: false };
    owner.current = scope;slots.current.clear();publishSlots();setSession(null);setConnection('connecting');setError(null);
    try {
      const next = await client.open();scope.session = next;
      if (!active(scope)) { close(scope);return; }
      setSession(next);setConnection('connected');
    } catch (failure) {
      if (active(scope)) { setConnection('lost');setError(failure instanceof Error ? failure.message : String(failure)); }
    }
  }, [enabled, close, publishSlots, client, active]);
  const call = useCallback(async <T>(operation: ExecutionOperation, invoke: (id: string) => Promise<ExecutionReply<T>>): Promise<ExecutionReply<T>> => {
    const scope = owner.current;
    if (!enabled || !scope || !active(scope) || !scope.session) throw new ExecutionApiError('session-lost', '明示的に再接続してください。', false);
    const lane = laneFor(operation);
    if (slots.current.has(lane)) throw new ExecutionApiError('busy', '同じ種類の前の要求が完了するまで待ってください。', false);
    const slot: Slot = { owner: scope, operation };slots.current.set(lane, slot);publishSlots();setError(null);
    try {
      const reply = await invoke(scope.session.id);
      if (!active(scope)) throw new ExecutionApiError('session-lost', '要求中に接続が変わりました。結果は不明です。', true);
      acceptState(scope, reply.state);return reply;
    } catch (failure) {
      if (active(scope)) {
        const known = failure instanceof ExecutionApiError && !failure.resultUnknown;
        if (failure instanceof ExecutionApiError && failure.state) acceptState(scope, failure.state);
        setError(failure instanceof Error ? failure.message : String(failure));
        if (!known) { close(scope);slots.current.clear();publishSlots();setSession(null);setConnection('lost'); }
      }
      throw failure;
    } finally {
      if (slots.current.get(lane) === slot) { slots.current.delete(lane);publishSlots(); }
    }
  }, [enabled, active, publishSlots, acceptState, close]);
  useEffect(() => {
    mounted.current = true;
    if (enabled) void connect();
    else { setSession(null);setConnection('disconnected');setError(null);slots.current.clear();publishSlots(); }
    const hide = () => {
      close(owner.current);slots.current.clear();publishSlots();setSession(null);setConnection('lost');setError('ページを離れました。再接続して計画を準備し直してください。');
    };
    const show = (event: PageTransitionEvent) => { if (event.persisted) hide(); };
    if (enabled) { window.addEventListener('pagehide', hide);window.addEventListener('pageshow', show); }
    return () => {
      mounted.current = false;close(owner.current);owner.current = null;slots.current.clear();
      window.removeEventListener('pagehide', hide);window.removeEventListener('pageshow', show);
    };
  }, [enabled, connect, close, publishSlots]);
  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(() => {
      const scope = owner.current;
      if (!scope || !active(scope) || !scope.session || slots.current.has('mutation') || slots.current.has('poll')) return;
      void call('status', id => client.status(id)).catch(() => {});
    }, 30_000);
    return () => clearInterval(timer);
  }, [enabled, active, call, client]);
  const getSession = useCallback(() => {
    const scope = owner.current;return scope && active(scope) ? scope.session : null;
  }, [active]);
  return { session, connection, connecting: connection === 'connecting', error, busy, polling, cancelling, connect, call, getSession };
}
