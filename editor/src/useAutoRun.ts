import { useCallback, useEffect, useRef, useState } from 'react';

export const AUTO_RUN_DELAY_MS = 200;

interface Options {
  // セッション・稼働版・編集中のグラフが一致する場合だけ有効な識別子。
  scope: string | null;
  inputKey: string;
  blocked: boolean;
  run: () => Promise<void>;
}

/** 実行中は最新1件だけを保持する。失敗した入力を自動で再試行しない。 */
export function useAutoRun(options: Options) {
  const [enabled, setEnabledState] = useState(false);
  const latest = useRef({ ...options, enabled });
  latest.current = { ...options, enabled };
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pending = useRef<{ key: string; due: number } | null>(null);
  const attempted = useRef<string | null>(null);
  const clearTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);
  const setEnabled = useCallback((value: boolean) => {
    latest.current.enabled = value;
    clearTimer(); pending.current = null; attempted.current = null;
    setEnabledState(value);
  }, [clearTimer]);

  // 手動実行も同じ入力の試行として記録し、待機中の重複実行を取り消す。
  const consume = useCallback(() => {
    const current = latest.current;
    attempted.current = current.scope ? JSON.stringify([current.scope, current.inputKey]) : null;
    pending.current = null; clearTimer();
  }, [clearTimer]);

  useEffect(() => {
    if (!enabled || options.scope === null) {
      clearTimer(); pending.current = null; attempted.current = null;
      return;
    }
    const key = JSON.stringify([options.scope, options.inputKey]);
    if (attempted.current === key) { clearTimer(); pending.current = null; return; }
    if (pending.current?.key !== key) pending.current = { key, due: Date.now() + AUTO_RUN_DELAY_MS };
    clearTimer();
    // 待機期限は入力変更時から数える。実行完了後は最新入力だけを処理する。
    if (options.blocked) return;
    timer.current = setTimeout(() => {
      timer.current = null;
      const current = latest.current;
      if (!current.enabled || current.blocked || current.scope === null
        || JSON.stringify([current.scope, current.inputKey]) !== key) return;
      attempted.current = key; pending.current = null;
      // エラー表示は手動実行と共通の呼出側に任せる。
      void current.run().catch(() => {});
    }, Math.max(0, pending.current.due - Date.now()));
    return clearTimer;
  }, [enabled, options.scope, options.inputKey, options.blocked, clearTimer]);

  useEffect(() => {
    const hide = () => setEnabled(false);
    window.addEventListener('pagehide', hide);
    return () => { window.removeEventListener('pagehide', hide); clearTimer(); };
  }, [setEnabled, clearTimer]);

  return { enabled, setEnabled, consume };
}
