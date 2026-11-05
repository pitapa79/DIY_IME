import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { PackageManager } from '../server/node-packages';
const { spawn } = vi.hoisted(() => ({ spawn: vi.fn() }));
vi.mock('node:child_process', () => ({ spawn }));
class FakeChild extends EventEmitter {
  stdin = new PassThrough(); stdout = new PassThrough(); stderr = new PassThrough(); kill = vi.fn();
  frames() {
    const data = this.stdin.read() as Buffer | null; if (!data) return [];
    const result: { id: string; operation: Record<string, unknown>; payload: Buffer }[] = [];
    let offset = 0;
    while (offset < data.length) {
      const json = data.readUInt32LE(offset), payload = Number(data.readBigUInt64LE(offset + 4)); offset += 12;
      const request = JSON.parse(data.subarray(offset, offset + json).toString()); offset += json;
      result.push({ ...request, payload: data.subarray(offset, offset + payload) }); offset += payload;
    }
    return result;
  }
  reply(id: string, data: unknown) { this.stdout.emit('data', Buffer.from(JSON.stringify({ id, ok: true, data }) + '\n')); }
}
let folder: string, child: FakeChild, manager: PackageManager;
beforeEach(() => {
  folder = mkdtempSync(join(tmpdir(), 'diyime-package-ipc-'));
  const config = join(folder, 'config.json'); writeFileSync(config, '{}');
  child = new FakeChild(); spawn.mockReturnValue(child);
  manager = new PackageManager({ executable: 'host-fixed-manager', config });
});
afterEach(async () => { const ended = manager.close(); child.emit('close', 0); await ended; rmSync(folder, { recursive: true }); vi.resetAllMocks(); });
it('uses only configured process arguments, binary payload and complete UTF8 lines', async () => {
  const pending = manager.call({ method: 'review' }, Buffer.from([0, 255, 0, 12]));
  const [frame] = child.frames(); expect(frame.payload).toEqual(Buffer.from([0, 255, 0, 12]));
  expect(spawn).toHaveBeenCalledWith('host-fixed-manager', [join(folder, 'config.json')], expect.objectContaining({ windowsHide: true, stdio: 'pipe' }));
  const bytes = Buffer.from(JSON.stringify({ id: frame.id, ok: true, data: '日本語' }) + '\n');
  const split = bytes.indexOf(Buffer.from('日')) + 1;
  child.stdout.emit('data', bytes.subarray(0, split)); child.stdout.emit('data', bytes.subarray(split));
  expect(await pending).toBe('日本語'); expect(child.kill).not.toHaveBeenCalled();
});
it('cancels through IPC and refuses reuse until the Host settles the original request', async () => {
  const abort = new AbortController();
  const settled = vi.fn();
  const result = manager.call({ method: 'review' }, Buffer.alloc(0), abort.signal).then(value => ({ value }), error => ({ error })).finally(settled);
  const [request] = child.frames(); abort.abort();
  expect(child.frames()[0].operation).toEqual({ method: 'cancel', requestId: request.id });
  await expect(manager.call({ method: 'list' })).rejects.toMatchObject({ code: 'package-busy' });
  await Promise.resolve(); expect(settled).not.toHaveBeenCalled();
  child.reply(request.id, { discarded: true }); expect(await result).toHaveProperty('error.code', 'aborted');
  const next = manager.call({ method: 'list' }); const [frame] = child.frames(); child.reply(frame.id, { entries: [] }); await next;
  expect(child.kill).not.toHaveBeenCalled();
});
it('holds pending ownership through malformed output until actual child close and never force-kills', async () => {
  const settled = vi.fn();
  const pending = manager.call({ method: 'list' }).catch(error => error).finally(settled);
  child.frames(); child.stdout.emit('data', Buffer.from('not-json\n'));
  const [close] = child.frames(); expect(close.operation.method).toBe('close');
  await Promise.resolve(); expect(settled).not.toHaveBeenCalled(); expect(child.kill).not.toHaveBeenCalled();
  child.emit('close', 1); expect(await pending).toMatchObject({ code: 'package-protocol' });
});
it('rejects a request over the fixed Host JSON limit without leaving a pending slot and uses EOF if close cannot fit', async () => {
  const config = join(folder, 'execution.json');writeFileSync(config, JSON.stringify({ requestBytes: 80, responseBytes: 2048 }));manager = new PackageManager({ executable: 'fixed-execution-host', config }, 'execution');
  await expect(manager.call({ method: 'prepare', graph: 'x'.repeat(100) })).rejects.toMatchObject({ code: 'host-request-limit' });
  const pending = manager.call({ method: 'status' });const [frame] = child.frames();child.reply(frame.id, {});await pending;
  const ended = manager.close();child.emit('close', 0);await ended;expect(manager.cleanExit).toBe(true);expect(child.kill).not.toHaveBeenCalled();
  child = new FakeChild();spawn.mockReturnValue(child);
  const tiny = join(folder, 'tiny.json');writeFileSync(tiny, JSON.stringify({ requestBytes: 1, responseBytes: 2048 }));manager = new PackageManager({ executable: 'fixed-execution-host', config: tiny }, 'execution');
  await expect(manager.call({ method: 'status' })).rejects.toMatchObject({ code: 'host-request-limit' });const draining = manager.close();expect(child.stdin.writableEnded).toBe(true);child.emit('close', 0);await draining;
});
