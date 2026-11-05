import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from './api';
import type { CoreSession, SessionResult, SessionState } from './model';

export type Operation = 'validate' | 'prepare' | 'activate' | 'run' | 'rollback';

/** 接続の所有権と応答順序を管理する。途中の切断後に状態変更を自動再試行しない。 */
export function useCoreSession(autoConnect = true) {
  const [session, setSession] = useState<CoreSession | null>(null);
  const [connecting, setConnecting] = useState(autoConnect), [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Operation | null>(null);
  const current = useRef<CoreSession | null>(null), owned = useRef<string | null>(null);
  const generation = useRef(0), inFlight = useRef<number | null>(null);

  const accept = useCallback((id: string, state: SessionState) => {
    if (current.current?.id !== id || state.revision < current.current.state.revision) return;
    const next = { id, state }; current.current = next; setSession(next);
  }, []);
  const lost = useCallback((id: string, message: string) => {
    if (current.current?.id !== id) return;
    current.current = null; setSession(null); setError(message);
  }, []);

  const connect = useCallback(async (onConnected?: () => void) => {
    const token = ++generation.current;
    inFlight.current = null; setBusy(null);
    const previous = owned.current; owned.current = null; current.current = null;
    setSession(null); setConnecting(true); setError(null);
    if (previous) await api.closeSession(previous);
    try {
      const next = await api.openSession();
      if (token !== generation.current) { void api.closeSession(next.id); return; }
      current.current = next; owned.current = next.id; setSession(next);
      // 新接続の所有権が確定した同じ段階でだけ復旧処理を行う。
      onConnected?.();
    } catch (failure) { if (token === generation.current) setError(failure instanceof Error ? failure.message : String(failure)); }
    finally { if (token === generation.current) setConnecting(false); }
  }, []);

  useEffect(() => {
    if (autoConnect) void connect();
    const dispose = () => {
      generation.current++; const id = owned.current; owned.current = null; current.current = null;
      if (id) void api.closeSession(id);
    };
    const show = (event: PageTransitionEvent) => { if (event.persisted) { setSession(null); setError('ページを復元しました。再接続して計画を準備し直してください。'); } };
    window.addEventListener('pagehide', dispose); window.addEventListener('pageshow', show);
    return () => { dispose(); window.removeEventListener('pagehide', dispose); window.removeEventListener('pageshow', show); };
  }, [connect, autoConnect]);

  useEffect(() => {
    const timer = setInterval(() => {
      const id = current.current?.id;
      if (!id || inFlight.current !== null) return;
      void api.sessionStatus(id).then(reply => accept(id, reply.state)).catch(failure => lost(id, failure instanceof Error ? failure.message : String(failure)));
    }, 30_000);
    return () => clearInterval(timer);
  }, [accept, lost]);

  const call = async <T,>(operation: Operation, invoke: (id: string) => Promise<SessionResult<T>>): Promise<SessionResult<T>> => {
    const id = current.current?.id;
    if (!id) throw new ApiError('session-lost', '常駐コアに再接続してください。');
    if (inFlight.current !== null) throw new ApiError('busy', '前の操作が完了するまで待ってください。');
    const token = generation.current;
    inFlight.current = token; setBusy(operation);
    try {
      const reply = await invoke(id);
      if (token !== generation.current) throw new ApiError('session-lost', '操作中に接続が変更されました。');
      accept(id, reply.state); return reply;
    } catch (failure) {
      if (token === generation.current) {
        if (failure instanceof ApiError && failure.state) accept(id, failure.state);
        else if (!(failure instanceof ApiError) || ['session-lost', 'core-unavailable', 'bridge-error'].includes(failure.code)) {
          lost(id, failure instanceof Error ? failure.message : String(failure));
        }
      }
      throw failure;
    } finally { if (inFlight.current === token) inFlight.current = null; if (token === generation.current) setBusy(null); }
  };
  return { session, connecting, error, busy, connect, call };
}
