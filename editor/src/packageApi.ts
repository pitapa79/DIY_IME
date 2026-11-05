import type { Definition } from './model';
export interface InstalledPackage { manifest: { packageId: string; displayName: string; version: string; node: Definition; capabilities: { id: string; binding: string }[] }; packageSha256: string; authorizationSha256: string }
export interface PackageCatalog { formatVersion: number; revision: string; entries: InstalledPackage[] }
export type PackageChange = 'new' | 'unchanged' | 'metadata' | 'upgrade' | 'downgrade' | 'sameVersionRebuild';
export interface PackageReview { reviewId: string; review: { expectedRevision: string; changes: { packageId: string; nodeKind: string; previousVersion: string | null; version: string; change: PackageChange; requiresPermissionReview: boolean }[] }; executionPermissionGranted: false }
async function call(path: string, init: RequestInit = {}, signal?: AbortSignal) {
  const response = await fetch('/api/node-packages' + path, { ...init, signal });
  const value = await response.json() as { ok: boolean; data?: unknown; error?: { code?: string; message?: string } };
  if (!response.ok || !value.ok) {
    const messages: Record<string, string> = {
      'package-version': '対応していないパッケージ形式または実行方式です。',
      'package-digest': 'パッケージが破損しているか、記載された情報と一致しません。',
      'package-definition': '記載されたノード定義と実装が一致しません。',
      'package-capability': 'この版では、パッケージが必要とする機能に対応していません。',
      'catalog-conflict': '導入済み一覧が変更されました。パッケージを選び直してください。',
      'catalog-io': '導入先に保存できません。一覧を再読み込みして状態を確認してください。',
      'manager-review': '確認結果を利用できません。パッケージを選び直してください。',
      'manager-cancelled': '検証を中止しました。',
      'worker-cancelled': '検証を中止しました。',
    };
    const code = value.error?.code ?? '';
    throw new Error(messages[code] ?? (code.startsWith('worker-') ? '隔離した検証を完了できませんでした。' : value.error?.message) ?? 'ノード管理を確認できません。');
  }
  return value.data;
}
const post = (value: unknown): RequestInit => ({ method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value) });
export const packageApi = {
  catalog: (signal?: AbortSignal) => call('', {}, signal) as Promise<{ available: boolean; catalog?: PackageCatalog }>,
  review: (files: File[], signal?: AbortSignal) => {
    const count = new ArrayBuffer(4); new DataView(count).setUint32(0, files.length, true);
    const parts: BlobPart[] = [count];
    for (const file of files) { const length = new ArrayBuffer(8); new DataView(length).setBigUint64(0, BigInt(file.size), true); parts.push(length, file); }
    return call('/review', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: new Blob(parts) }, signal) as Promise<PackageReview>;
  },
  commit: (reviewId: string, allowDowngrade: boolean, allowSameVersionRebuild: boolean, signal?: AbortSignal) => call('/commit', post({ reviewId, allowDowngrade, allowSameVersionRebuild }), signal) as Promise<{ catalog: PackageCatalog }>,
  discard: (reviewId: string, signal?: AbortSignal) => call('/discard', post({ reviewId }), signal),
  cancel: () => call('/cancel', post({})),
};