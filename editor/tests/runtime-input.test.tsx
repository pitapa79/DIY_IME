// @vitest-environment jsdom
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useRuntimeInput } from '../src/useRuntimeInput';

const storage = 'diy-ime2.editor.runtime-input.v1';
const consent = 'diy-ime2.editor.remember-runtime-input.v1';
beforeEach(() => localStorage.clear());
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it('preserves invalid draft text exactly for repair instead of overwriting it on restore', () => {
  for (const text of ['prefix\ud800suffix', '𠮷'.repeat(4097)]) {
    const saved = { revision: 7, text, mode: 'compose' };
    localStorage.setItem(consent, 'true');
    localStorage.setItem(storage, JSON.stringify(saved));
    const hook = renderHook(useRuntimeInput);
    expect(hook.result.current.input).toEqual(saved);
    expect(hook.result.current.error).toBeTruthy();
    expect(JSON.parse(localStorage.getItem(storage)!)).toEqual(saved);
    hook.unmount();
  }
});

it('still rejects malformed saved structure, revision or mode', () => {
  for (const saved of [null, [], { revision: 0, text: 'a', mode: 'compose' }, { revision: 1.5, text: 'a', mode: 'compose' }, { revision: 1, text: 3, mode: 'compose' }, { revision: 1, text: 'a', mode: 'other' }]) {
    localStorage.setItem(consent, 'true');
    localStorage.setItem(storage, JSON.stringify(saved));
    const hook = renderHook(useRuntimeInput);
    expect(hook.result.current.input).toEqual({ revision: 1, text: '', mode: 'compose' });
    expect(hook.result.current.error).toBeUndefined();
    hook.unmount();
  }
});

it('clears the text error after an explicit repair and increments the saved revision', () => {
  localStorage.setItem(consent, 'true');
    localStorage.setItem(storage, JSON.stringify({ revision: 7, text: 'prefix\ud800suffix', mode: 'finalize' }));
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.update({ text: 'prefix𠮷suffix' }));
  const expected = { revision: 8, text: 'prefix𠮷suffix', mode: 'finalize' };
  expect(hook.result.current.input).toEqual(expected);
  expect(hook.result.current.error).toBeUndefined();
  expect(JSON.parse(localStorage.getItem(storage)!)).toEqual(expected);
});

it('retains exhausted input until explicitly restarted after a new core connection', () => {
  const saved = { revision: Number.MAX_SAFE_INTEGER, text: 'prefix\ud800suffix', mode: 'finalize' };
  localStorage.setItem(consent, 'true');
    localStorage.setItem(storage, JSON.stringify(saved));
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.update({ text: 'replacement' }));
  expect(hook.result.current.input).toEqual(saved);
  expect(hook.result.current.exhausted).toBe(true);
  act(() => hook.result.current.restartRevision());
  expect(hook.result.current.input).toEqual({ ...saved, revision: 1 });
  expect(hook.result.current.error).toContain('読み取れない文字');
  act(() => hook.result.current.update({ text: 'prefix𠮷suffix' }));
  expect(hook.result.current.input).toEqual({ revision: 2, text: 'prefix𠮷suffix', mode: 'finalize' });
  expect(hook.result.current.exhausted).toBe(false);
  expect(hook.result.current.error).toBeUndefined();
  act(() => hook.result.current.restartRevision());
  expect(hook.result.current.input.revision).toBe(2);
  expect(JSON.parse(localStorage.getItem(storage)!)).toEqual(hook.result.current.input);
});

it('does not restore an unconsented legacy body and removes the old saved text', () => {
  localStorage.setItem(storage, JSON.stringify({ revision: 9, text: 'legacy-canary', mode: 'finalize' }));
  const reads = vi.spyOn(Storage.prototype, 'getItem');
  const hook = renderHook(useRuntimeInput);
  expect(hook.result.current.input).toEqual({ revision: 1, text: '', mode: 'compose' });
  expect(hook.result.current.remember).toBe(false);
  expect(reads.mock.calls.some(([key]) => key === storage)).toBe(false);
  expect(localStorage.getItem(storage)).toBeNull();
});

it('keeps typed text only in memory by default and does not restore it after remount', () => {
  const writes = vi.spyOn(Storage.prototype, 'setItem');
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.update({ text: 'unsaved-canary' }));
  expect(hook.result.current.input.text).toBe('unsaved-canary');
  expect(writes.mock.calls.some(([, value]) => value.includes('unsaved-canary'))).toBe(false);
  expect(localStorage.getItem(storage)).toBeNull();
  hook.unmount();
  const next = renderHook(useRuntimeInput);
  expect(next.result.current.input.text).toBe('');
});

it('saves only after explicit selection and erases saved text on revocation without losing live input', () => {
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.update({ text: 'chosen-canary' }));
  act(() => hook.result.current.setRememberInput(true));
  expect(hook.result.current.remember).toBe(true);
  expect(JSON.parse(localStorage.getItem(storage)!)).toEqual(hook.result.current.input);
  hook.unmount();
  const restored = renderHook(useRuntimeInput);
  expect(restored.result.current.input.text).toBe('chosen-canary');
  act(() => restored.result.current.setRememberInput(false));
  expect(restored.result.current.input.text).toBe('chosen-canary');
  expect(localStorage.getItem(storage)).toBeNull();
  expect(localStorage.getItem(consent)).toBeNull();
  act(() => restored.result.current.update({ text: 'new-unsaved-canary' }));
  expect(localStorage.getItem(storage)).toBeNull();
});

it('fails closed when recording consent is denied and reports failed deletion', () => {
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.update({ text: 'memory-canary' }));
  const writes = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('denied'); });
  act(() => hook.result.current.setRememberInput(true));
  expect(hook.result.current.remember).toBe(false);
  expect(hook.result.current.stored).toBe(false);
  expect(localStorage.getItem(storage)).toBeNull();
  writes.mockRestore();
  act(() => hook.result.current.setRememberInput(true));
  const removes = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => { throw new Error('denied'); });
  act(() => hook.result.current.setRememberInput(false));
  expect(hook.result.current.remember).toBe(false);
  expect(hook.result.current.stored).toBe(false);
  expect(hook.result.current.input.text).toBe('memory-canary');
  removes.mockRestore();
  act(() => hook.result.current.update({ text: 'repair-canary' }));
  expect(localStorage.getItem(storage)).toBeNull();
});

it('honors another tab revoking persistence without clearing the live buffer', () => {
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.setRememberInput(true));
  act(() => hook.result.current.update({ text: 'tab-canary' }));
  localStorage.removeItem(consent);
  act(() => window.dispatchEvent(new StorageEvent('storage', { key: consent, newValue: null, storageArea: localStorage })));
  expect(hook.result.current.remember).toBe(false);
  expect(hook.result.current.input.text).toBe('tab-canary');
  expect(localStorage.getItem(storage)).toBeNull();
});

it('revokes consent before deleting the body so a body deletion failure cannot restore it', () => {
  const hook = renderHook(useRuntimeInput);
  act(() => hook.result.current.setRememberInput(true));
  act(() => hook.result.current.update({ text: 'deletion-failure-canary' }));
  const remove = Storage.prototype.removeItem;
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key: string) {
    if (key === storage) throw new Error('body deletion denied');
    return remove.call(this, key);
  });
  act(() => hook.result.current.setRememberInput(false));
  expect(hook.result.current.stored).toBe(false);
  expect(localStorage.getItem(consent)).toBeNull();
  expect(localStorage.getItem(storage)).toContain('deletion-failure-canary');
  hook.unmount();
  const next = renderHook(useRuntimeInput);
  expect(next.result.current.input.text).toBe('');
  expect(next.result.current.remember).toBe(false);
  expect(next.result.current.stored).toBe(false);
});