import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { decodeTracePage, emptyTrace, importTrace, mergeTrace, TRANSFER_BYTES, type TracePage } from './executionTrace';
export interface TraceConnection { scope: object; start(): Promise<TracePage>; startDetailed?(): Promise<TracePage>; read(after: string, limit: number): Promise<TracePage>; stop(): Promise<TracePage>; clear(): Promise<TracePage> }
export interface FrozenTrace { trace: TracePage; imported: boolean }
export function useExecutionTrace(connection: TraceConnection | null, blocked: boolean) {
  const policy = useRef(blocked);policy.current = blocked;
  const [trace, setTrace] = useState(emptyTrace), live = useRef(trace), generation = useRef(0);
  const [revision, setRevision] = useState(0);
  const [watching, setWatching] = useState(false), [busy, setBusy] = useState(false), [issue, setIssue] = useState<string | null>(null);
  const [frozen, setFrozen] = useState<[FrozenTrace | null, FrozenTrace | null]>([null, null]);
  const accept = (value: TracePage) => { live.current = value; setTrace(value); };
  useLayoutEffect(() => {
    generation.current++; accept(emptyTrace()); setWatching(false); setBusy(false); setIssue(null);
    if (blocked) setFrozen([null, null]);
    return () => { generation.current++; };
  }, [connection?.scope, blocked]);
  useEffect(() => {
    if (!watching || !connection || blocked) return;
    const version = generation.current; let cancelled = false, timer: ReturnType<typeof setTimeout>;
    const valid = () => !cancelled && version === generation.current;
    async function poll() {
      try {
        const cursor = live.current.events.at(-1)?.sequence ?? '0';
        const page = await connection!.read(cursor, 256); if (!valid()) return;
        const merged = mergeTrace(live.current, page); accept(merged);
        const caught = page.events.length === 0 || BigInt(merged.events.at(-1)?.sequence ?? '0') >= BigInt(page.nextSequence);
        if (!page.recording && caught) { setWatching(false); return; }
        timer = setTimeout(() => void poll(), caught ? 150 : 0);
      } catch { if (valid()) { setIssue('記録を取得できません。接続・保護状態を確認してください。'); setWatching(false); } }
    }
    void poll(); return () => { cancelled = true; clearTimeout(timer); };
  }, [watching, connection, blocked, revision]);
  async function command(action: 'start' | 'startDetailed' | 'stop' | 'clear') {
    if (!connection || policy.current || busy) return;
    const operation = connection[action]; if (!operation) return;
    const version = ++generation.current; setRevision(n => n + 1); setWatching(false); setBusy(true); setIssue(null);
    if (action === 'start' || action === 'startDetailed' || action === 'clear') accept(emptyTrace());
    if (action === 'clear') setFrozen([null, null]);
    try {
      const page = await operation(); if (version !== generation.current) return;
      accept(mergeTrace(live.current, page)); setWatching(action !== 'clear' && page.recordingId !== null);
    } catch { if (version === generation.current) setIssue('記録操作の完了を確認できません。保護または再接続で確認してください。'); }
    finally { if (version === generation.current) setBusy(false); }
  }
  function freeze(slot: 0 | 1) {
    if (policy.current || !live.current.recordingId) return;
    const snapshot = decodeTracePage(live.current, true);
    setFrozen(previous => { const next: typeof previous = [...previous]; next[slot] = { trace: snapshot, imported: false }; return next; });
  }
  async function load(file: File, slot: 0 | 1, expected = generation.current) {
    if(policy.current || expected!==generation.current)return false;
    if (policy.current || file.size > TRANSFER_BYTES) { setIssue('読込ファイルは16MiB以内の実行記録を選んでください。'); return false; }
    const version = generation.current;
    try {
      const text = await file.text(); if (version !== generation.current || policy.current) return false;
      const snapshot = importTrace(text); if (version !== generation.current || policy.current) return false;
      setFrozen(previous => { const next: typeof previous = [...previous]; next[slot] = { trace: snapshot, imported: true }; return next; }); setIssue(null); return true;
    } catch { if (version === generation.current) setIssue('実行記録の形式または上限を確認してください。');return false; }
  }
  function retry() { if (!blocked && connection && !busy) { generation.current++;setRevision(n => n + 1);setIssue(null);setWatching(true); } }
  return { trace, frozen, watching, busy, issue, command, freeze, load, retry, importVersion:()=>generation.current };
}
