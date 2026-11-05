import { useEffect, useRef, useState } from 'react';
import { packageApi, type PackageCatalog, type PackageChange, type PackageReview } from './packageApi';
const changeLabels: Record<PackageChange, string> = { new: '新規導入', unchanged: '導入済みと同じ', metadata: '表示情報の変更', upgrade: '更新', downgrade: '以前の版へ戻す', sameVersionRebuild: '同じ版番号で実装が変更' };
export default function NodePackages({ onClose }: { onClose: () => void }) {
  const [catalog, setCatalog] = useState<PackageCatalog>();
  const [available, setAvailable] = useState<boolean>();
  const [review, setReview] = useState<PackageReview>();
  const [busy, setBusy] = useState<'loading' | 'reviewing' | 'committing'>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [allowDowngrade, setAllowDowngrade] = useState(false), [allowRebuild, setAllowRebuild] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const active = useRef<AbortController | undefined>(undefined), alive = useRef(true);
  const files = useRef<HTMLInputElement>(null);
  const operate = async (kind: 'loading' | 'reviewing' | 'committing', action: (signal: AbortSignal) => Promise<void>) => {
    const controller = new AbortController(); active.current = controller; setBusy(kind); setError(undefined); setNotice(undefined); setCancelling(false);
    try { await action(controller.signal); } catch (cause) { if (alive.current && !controller.signal.aborted) setError(cause instanceof Error ? cause.message : '処理を確認できません。'); }
    finally { if (active.current === controller) { if (alive.current) { setBusy(undefined); setCancelling(false); } active.current = undefined; } }
  };
  const load = () => operate('loading', async signal => { const result = await packageApi.catalog(signal); if (!alive.current || signal.aborted) return; setAvailable(result.available); setCatalog(result.catalog); });
  useEffect(() => { alive.current = true; void load(); return () => { alive.current = false; active.current?.abort(); }; }, []); // Host retains its slot until real settlement.
  const select = (selected: File[]) => {
    if (!selected.length || busy) return;
    setReview(undefined); setAllowDowngrade(false); setAllowRebuild(false);
    void operate('reviewing', async signal => { const result = await packageApi.review(selected, signal); if (alive.current && !signal.aborted) setReview(result); });
  };
  const downgrade = review?.review.changes.some(item => item.change === 'downgrade') ?? false;
  const rebuild = review?.review.changes.some(item => item.change === 'sameVersionRebuild') ?? false;
  const confirm = () => {
    if (!review || busy) return;
    const id = review.reviewId;
    void operate('committing', async signal => {
      try {
        const result = await packageApi.commit(id, allowDowngrade, allowRebuild, signal);
        if (alive.current && !signal.aborted) { setCatalog(result.catalog); setReview(undefined); setNotice('選択したノードを導入しました。実行許可はまだ付与していません。'); }
      } catch (cause) { if (alive.current) setReview(undefined); throw cause; }
    });
  };
  const discard = () => {
    if (!review || busy) return;
    const id = review.reviewId;
    void operate('loading', async signal => { await packageApi.discard(id, signal); if (alive.current) setReview(undefined); });
  };
  const cancel = async () => { setCancelling(true); try { await packageApi.cancel(); } catch (cause) { if (alive.current) setError(cause instanceof Error ? cause.message : '中止要求を送信できません。'); } };
  return <div className="modal-backdrop"><section className="dialog node-packages-dialog" role="dialog" aria-modal="true" aria-labelledby="node-packages-title" onKeyDown={event => { if (event.key === 'Escape') onClose(); }}>
    <div className="dialog-heading"><h2 id="node-packages-title">ノードの導入</h2><button className="button small" autoFocus onClick={onClose}>閉じる</button></div>
    <p>ノードのパッケージを複数選んで、内容を確認してからまとめて導入できます。</p>
    <p className="field-note">導入後は「導入済みノードで実行」でグラフを確認し、実行と入力経路を承認してください。</p>
    {error && <p role="alert" className="field-error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {available === false && <p>このアプリではノード管理が構成されていません。</p>}
    <div className="package-actions">
      <button className="button" disabled={!available || !!busy} onClick={() => files.current?.click()}>パッケージを選ぶ</button>
      <button className="button" disabled={!!busy} onClick={() => void load()}>一覧を再読み込み</button>
      <input ref={files} hidden type="file" accept=".diynode" multiple aria-label="導入するノードパッケージ" onChange={event => { select(Array.from(event.target.files ?? [])); event.target.value = ''; }} />
    </div>
    {busy && <div role="status"><p>{cancelling ? '中止しています。処理が終了するまでお待ちください。' : busy === 'reviewing' ? '選択したパッケージを検証しています…' : busy === 'committing' ? 'ノードを導入しています…' : '一覧を確認しています…'}</p>{busy === 'reviewing' && <button className="button" disabled={cancelling} onClick={() => void cancel()}>検証を中止</button>}</div>}
    {review && <section aria-labelledby="package-review-title"><h3 id="package-review-title">導入内容の確認</h3>
      <table className="package-table"><thead><tr><th>ノード</th><th>版</th><th>変更</th><th>実行許可</th></tr></thead><tbody>{review.review.changes.map(item => <tr key={item.packageId}><td><strong>{item.nodeKind}</strong><small>{item.packageId}</small></td><td>{item.previousVersion ? item.previousVersion + ' → ' : ''}{item.version}</td><td>{changeLabels[item.change]}</td><td>{item.requiresPermissionReview ? '別途確認が必要' : '実装・要求権限の変更なし'}</td></tr>)}</tbody></table>
      {downgrade && <label className="package-confirm"><input type="checkbox" checked={allowDowngrade} disabled={!!busy} onChange={event => setAllowDowngrade(event.target.checked)} />以前の版へ戻すことを確認しました</label>}
      {rebuild && <label className="package-confirm"><input type="checkbox" checked={allowRebuild} disabled={!!busy} onChange={event => setAllowRebuild(event.target.checked)} />同じ版番号で実装が変更されていることを確認しました</label>}
      <p>導入だけでは実行やネットワーク接続を許可しません。</p>
      <div className="package-actions"><button className="button primary" disabled={!!busy || (downgrade && !allowDowngrade) || (rebuild && !allowRebuild)} onClick={confirm}>確認して一括導入</button><button className="button" disabled={!!busy} onClick={discard}>確認を破棄</button></div>
    </section>}
    <h3>導入済みノード</h3>
    {catalog?.entries.length ? <ul className="package-list">{catalog.entries.map(item => <li key={item.manifest.packageId}><strong>{item.manifest.displayName}</strong><span>{item.manifest.version}</span><code>{item.manifest.node.kind}@{item.manifest.node.version}</code></li>)}</ul> : !busy && available && <p>導入済みのパッケージはありません。</p>}
  </section></div>;
}