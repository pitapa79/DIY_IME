import { useEffect, useRef, useState } from 'react';
import { runtimeInputError, type RuntimeInput } from './model';

const STORAGE = 'diy-ime2.editor.runtime-input.v1';
const CONSENT = 'diy-ime2.editor.remember-runtime-input.v1';
const emptyInput = (): RuntimeInput => ({ revision: 1, text: '', mode: 'compose' });
function restore(): { input: RuntimeInput; remember: boolean } {
  try {
    if (localStorage.getItem(CONSENT) !== 'true') return { input: emptyInput(), remember: false };
    const value: unknown = JSON.parse(localStorage.getItem(STORAGE) ?? 'null');
    if (value && typeof value === 'object' && !Array.isArray(value)) {
      const input = value as RuntimeInput;
      // 明示的に保存した下書きだけ、本文の問題を復元後に修正できるよう残す。
      if (typeof input.text === 'string' && !runtimeInputError({ ...input, text: '' })) {
        return { input: { revision: input.revision, text: input.text, mode: input.mode }, remember: true };
      }
    }
    return { input: emptyInput(), remember: true };
  } catch { return { input: emptyInput(), remember: false }; }
}

/** 全input.textノードの入力バッファ。永続保存は利用者が選んだ場合だけ。 */
export function useRuntimeInput() {
  const [initial] = useState(restore);
  const [input, setInput] = useState<RuntimeInput>(initial.input);
  const [remember, setRemember] = useState(initial.remember);
  const live = useRef(input); live.current = input;
  const [stored, setStored] = useState(true), [revisionError, setRevisionError] = useState<string>();
  useEffect(() => {
    try {
      if (remember && localStorage.getItem(CONSENT) === 'true') {
        localStorage.setItem(STORAGE, JSON.stringify(input));
      } else {
        localStorage.removeItem(CONSENT);
        localStorage.removeItem(STORAGE);
        if (remember) setRemember(false);
      }
      setStored(true);
    } catch { setStored(false); }
  }, [input, remember]);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.storageArea === localStorage && (event.key === CONSENT || event.key === null)
        && event.newValue !== 'true') setRemember(false);
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);
  const setRememberInput = (enabled: boolean) => {
    try {
      if (enabled) localStorage.setItem(CONSENT, 'true');
      else { localStorage.removeItem(CONSENT); localStorage.removeItem(STORAGE); }
      setStored(true);
    } catch {
      setStored(false);
      // 許可の記録に失敗した場合は本文の保存を開始しない。
      if (enabled) return;
    }
    setRemember(enabled);
  };
  const update = (patch: Partial<Pick<RuntimeInput, 'text' | 'mode'>>) => {
    const current = live.current, next = { ...current, ...patch };
    if (next.text === current.text && next.mode === current.mode) return;
    if (current.revision === Number.MAX_SAFE_INTEGER) { setRevisionError('入力の更新番号が上限に達しました。'); return; }
    next.revision++; live.current = next; setInput(next); setRevisionError(undefined);
  };
  const restartRevision = () => {
    if (live.current.revision !== Number.MAX_SAFE_INTEGER) return;
    const next = { ...live.current, revision: 1 };
    live.current = next; setInput(next); setRevisionError(undefined);
  };
  return { input, live, update, restartRevision, remember, setRememberInput, exhausted: input.revision === Number.MAX_SAFE_INTEGER, stored, error: revisionError ?? runtimeInputError(input) };
}
