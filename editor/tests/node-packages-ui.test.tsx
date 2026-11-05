// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import NodePackages from '../src/NodePackages';
import { packageApi } from '../src/packageApi';
vi.mock('../src/packageApi', () => ({ packageApi: { catalog: vi.fn(), review: vi.fn(), commit: vi.fn(), discard: vi.fn(), cancel: vi.fn() } }));
const empty = { formatVersion: 1, revision: '0', entries: [] };
beforeEach(() => { vi.resetAllMocks(); vi.mocked(packageApi.catalog).mockResolvedValue({ available: true, catalog: empty }); });
afterEach(cleanup);
it('reviews multiple files and requires explicit downgrade/rebuild confirmation before one batch commit', async () => {
  vi.mocked(packageApi.review).mockResolvedValue({ reviewId: 'a'.repeat(32), executionPermissionGranted: false, review: { expectedRevision: '1', changes: [
    { packageId: 'one', nodeKind: '<img src=x>', version: '1.0.0', previousVersion: '2.0.0', change: 'downgrade', requiresPermissionReview: true },
    { packageId: 'two', nodeKind: 'second', version: '1.0.0', previousVersion: '1.0.0', change: 'sameVersionRebuild', requiresPermissionReview: true },
  ] } });
  vi.mocked(packageApi.commit).mockResolvedValue({ catalog: { ...empty, revision: '2' } });
  const { container } = render(<NodePackages onClose={vi.fn()} />);
  await waitFor(() => expect((screen.getByRole('button', { name: 'パッケージを選ぶ' }) as HTMLButtonElement).disabled).toBe(false));
  const files = [new File(['one'], 'one.diynode'), new File(['two'], 'two.diynode')];
  fireEvent.change(screen.getByLabelText('導入するノードパッケージ'), { target: { files } });
  await screen.findByRole('heading', { name: '導入内容の確認' });
  expect(packageApi.review).toHaveBeenCalledWith(files, expect.any(AbortSignal));
  expect(container.querySelector('img')).toBeNull();
  const button = screen.getByRole('button', { name: '確認して一括導入' }) as HTMLButtonElement;
  expect(button.disabled).toBe(true);
  fireEvent.click(screen.getByLabelText('以前の版へ戻すことを確認しました')); expect(button.disabled).toBe(true);
  fireEvent.click(screen.getByLabelText('同じ版番号で実装が変更されていることを確認しました')); expect(button.disabled).toBe(false);
  fireEvent.click(button);
  await screen.findByText('選択したノードを導入しました。実行許可はまだ付与していません。');
  expect(packageApi.commit).toHaveBeenCalledExactlyOnceWith('a'.repeat(32), true, true, expect.any(AbortSignal));
});
it('keeps the review slot busy after cancellation until the Host operation actually settles', async () => {
  let reject!: (error: Error) => void;
  vi.mocked(packageApi.review).mockImplementation(() => new Promise((_, no) => { reject = no; }));
  vi.mocked(packageApi.cancel).mockResolvedValue({ cancelRequested: true });
  render(<NodePackages onClose={vi.fn()} />);
  await waitFor(() => expect((screen.getByRole('button', { name: 'パッケージを選ぶ' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.change(screen.getByLabelText('導入するノードパッケージ'), { target: { files: [new File(['one'], 'one.diynode')] } });
  fireEvent.click(await screen.findByRole('button', { name: '検証を中止' }));
  await screen.findByText('中止しています。処理が終了するまでお待ちください。');
  expect((screen.getByRole('button', { name: 'パッケージを選ぶ' }) as HTMLButtonElement).disabled).toBe(true);
  expect(packageApi.commit).not.toHaveBeenCalled();
  reject(new Error('検証を中止しました'));
  await screen.findByRole('alert');
  await waitFor(() => expect((screen.getByRole('button', { name: 'パッケージを選ぶ' }) as HTMLButtonElement).disabled).toBe(false));
});
it('shows unavailable configuration without pretending installed Nodes can execute', async () => {
  vi.mocked(packageApi.catalog).mockResolvedValue({ available: false });
  render(<NodePackages onClose={vi.fn()} />);
  await screen.findByText('このアプリではノード管理が構成されていません。');
  expect((screen.getByRole('button', { name: 'パッケージを選ぶ' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText('導入後は「導入済みノードで実行」でグラフを確認し、実行と入力経路を承認してください。')).toBeTruthy();
});