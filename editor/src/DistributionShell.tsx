import { useEffect, useState, type ReactNode } from 'react';
/** Application liveness is separate from permission to execute a graph. */
export function DistributionShell({ children }: { children: ReactNode }) {
  const [ended, setEnded] = useState(false), [error, setError] = useState<string | null>(null), [closing, setClosing] = useState(false);
  useEffect(() => {
    if (ended) return;
    let disposed = false;
    const beat = async () => { try { const response = await fetch('/api/app/heartbeat', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });if (!response.ok) throw Error(); }
      catch { if (!disposed) { setError('接続が終了しました。アプリを起動し直してください。');setEnded(true); } } };
    void beat();const timer = setInterval(() => { void beat(); }, 10000);
    return () => { disposed = true;clearInterval(timer); };
  }, [ended]);
  const close = async () => {
    if (closing) return;setClosing(true);
    try { const response = await fetch('/api/app/close', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });if (!response.ok) throw Error();setEnded(true); }
    catch { setError('終了を確認できませんでした。画面を閉じると接続が失効し、処理の停止を待ちます。'); }
    finally { setClosing(false); }
  };
  if (ended) return <main><h1>DIY_IME2</h1><p>{error ?? '終了しました。画面を閉じてください。'}</p></main>;
  return <>{children}<div className="distribution-controls">{error && <p role="alert">{error}</p>}<button className="button" disabled={closing} onClick={() => void close()}>{closing ? '終了しています…' : 'アプリを終了'}</button></div></>;
}