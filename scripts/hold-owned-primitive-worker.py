"""Bounded test-only fault on exact owned Worker handles; never production sync.

Registers obtained to confirm suspension are wiped and never emitted. Image,
workspace, digest and live process/thread ownership are checked before acting.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import importlib.util
import json
import msvcrt
import os
from pathlib import Path
import sys
import threading
import time

spec = importlib.util.spec_from_file_location(
    'hold_job_base', Path(__file__).with_name('measure-windows-benchmark-job.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
api = base.api
open_process = api('OpenProcess', w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
image_name = api('QueryFullProcessImageNameW', w.BOOL, w.HANDLE, w.DWORD,
                 w.LPWSTR, c.POINTER(w.DWORD))
process_id = api('GetProcessId', w.DWORD, w.HANDLE)
process_times = api('GetProcessTimes', w.BOOL, w.HANDLE,
                    *([c.POINTER(w.FILETIME)] * 4))
machine = api('IsWow64Process2', w.BOOL, w.HANDLE, c.POINTER(w.WORD), c.POINTER(w.WORD))
snapshot = api('CreateToolhelp32Snapshot', w.HANDLE, w.DWORD, w.DWORD)
open_thread = api('OpenThread', w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
thread_process = api('GetProcessIdOfThread', w.DWORD, w.HANDLE)
suspend = api('SuspendThread', w.DWORD, w.HANDLE)
resume = api('ResumeThread', w.DWORD, w.HANDLE)
context = api('GetThreadContext', w.BOOL, w.HANDLE, c.c_void_p)
peek_pipe = api('PeekNamedPipe', w.BOOL, w.HANDLE, c.c_void_p, w.DWORD,
                c.POINTER(w.DWORD), c.POINTER(w.DWORD), c.POINTER(w.DWORD))


class ThreadEntry(c.Structure):
    _fields_ = [('size', w.DWORD), ('usage', w.DWORD), ('tid', w.DWORD),
                ('pid', w.DWORD), ('priority', w.LONG), ('delta', w.LONG), ('flags', w.DWORD)]


first_thread = api('Thread32First', w.BOOL, w.HANDLE, c.POINTER(ThreadEntry))
next_thread = api('Thread32Next', w.BOOL, w.HANDLE, c.POINTER(ThreadEntry))


def digest_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as file:
        for part in iter(lambda: file.read(1024 * 1024), b''):
            digest.update(part)
    return digest.hexdigest()


def alive(handle):
    result = base.wait(handle, 0)
    if result not in (0, 258):
        raise c.WinError(c.get_last_error())
    return result == 258


def checked_worker(pid, root, digest):
    root = Path(root).resolve(strict=True)
    if root.name != 'workers' or not root.parent.name.startswith(
            ('diyime-primitive-retention-', 'diyime-retained-host-')):
        raise ValueError('private benchmark workspace required')
    if not isinstance(pid, int) or pid <= 0 or len(digest) != 64 or any(
            char not in '0123456789abcdef' for char in digest):
        raise ValueError('invalid expected process identity')
    handle = base.check(open_process(0x1000 | 0x100000 | 0x1, False, pid))
    try:
        if not alive(handle):
            raise ValueError('original Worker already ended')
        buffer, length = c.create_unicode_buffer(32768), w.DWORD(32768)
        base.check(image_name(handle, 0, buffer, c.byref(length)))
        path = Path(buffer.value).resolve(strict=True)
        if root not in path.parents or path.name != 'worker.exe' or not path.parent.name.startswith('DIYIME2.Worker.'):
            raise ValueError('process is not an exact workspace Worker')
        if digest_file(path) != digest:
            raise ValueError('Worker image digest differs')
        if c.sizeof(c.c_void_p) != 8:
            raise ValueError('native AMD64 helper required')
        guest_machine, native_machine = w.WORD(), w.WORD()
        base.check(machine(handle, c.byref(guest_machine), c.byref(native_machine)))
        if guest_machine.value != 0 or native_machine.value != 0x8664:
            raise ValueError('native AMD64 Worker required')
        times = [w.FILETIME() for _ in range(4)]
        base.check(process_times(handle, *(c.byref(value) for value in times)))
        if process_id(handle) != pid or not alive(handle):
            raise ValueError('original Worker identity expired')
        created = times[0].dwLowDateTime | (times[0].dwHighDateTime << 32)
        return handle, str(created)
    except BaseException:
        base.close(handle)
        raise


def thread_ids(pid):
    handle = snapshot(4, 0)  # TH32CS_SNAPTHREAD, read-only system snapshot.
    if handle == c.c_void_p(-1).value or not handle:
        raise c.WinError(c.get_last_error())
    try:
        row = ThreadEntry()
        row.size = c.sizeof(row)
        result, ids = first_thread(handle, c.byref(row)), set()
        while result:
            if row.pid == pid:
                ids.add(row.tid)
            row.size = c.sizeof(row)
            result = next_thread(handle, c.byref(row))
        if c.get_last_error() != 18:  # ERROR_NO_MORE_FILES
            raise c.WinError(c.get_last_error())
        return ids
    finally:
        base.close(handle)


def confirm_suspended(handle):
    # Native AMD64 CONTEXT has six DWORD64 home fields before ContextFlags;
    # require16-byte alignment and reserve more than its fixed1232-byte size.
    storage = c.create_string_buffer(4096 + 15)
    address = (c.addressof(storage) + 15) & ~15
    c.c_uint32.from_address(address + 48).value = 0x100001  # AMD64|CONTROL
    try:
        base.check(context(handle, address))
    finally:
        c.memset(c.addressof(storage), 0, c.sizeof(storage))


class HeldThreads:
    def __init__(self, process):
        self.process = process  # Borrowed original checked handle, never a PID action.
        self.pid = base.check(process_id(process))
        self.threads = {}
        self.forced_cleanup = False
        try:
            for _ in range(8):
                if not alive(process):
                    raise RuntimeError('original Worker ended before hold')
                for tid, (handle, _) in list(self.threads.items()):
                    result = base.wait(handle, 0)
                    if result == 0:
                        base.close(handle)
                        del self.threads[tid]
                    elif result != 258:
                        raise c.WinError(c.get_last_error())
                ids = thread_ids(self.pid)
                if not ids or len(ids | self.threads.keys()) > 128:
                    raise RuntimeError('invalid bounded Worker thread set')
                missing = ids - self.threads.keys()
                if not missing:
                    return
                for tid in sorted(missing):
                    thread = base.check(open_thread(0x2 | 0x8 | 0x800 | 0x100000, False, tid))
                    try:
                        # Immutable thread association was checked while the
                        # original process is still live, so PID reuse cannot
                        # qualify another process's thread for this action.
                        if thread_process(thread) != self.pid or not alive(process):
                            raise RuntimeError('thread no longer belongs to original live Worker')
                        previous = suspend(thread)
                        if previous == 0xffffffff:
                            raise c.WinError(c.get_last_error())
                        self.threads[tid] = (thread, previous)
                        thread = None
                        if previous:
                            raise RuntimeError('Worker thread was already suspended')
                        confirm_suspended(self.threads[tid][0])
                    finally:
                        if thread:
                            base.close(thread)
            raise RuntimeError('Worker thread set did not settle within bound')
        except BaseException:
            self.release()
            raise

    def release(self):
        released = 0
        try:
            for handle, previous in reversed(list(self.threads.values())):
                if alive(self.process) and base.wait(handle, 0) != 0:
                    observed = resume(handle)  # Undo ONLY our single increment.
                    if observed != previous + 1:
                        self.forced_cleanup = True
                        base.check(base.kill(self.process, 87))
                        if base.wait(self.process, 5000) != 0:
                            raise RuntimeError('owned Worker cleanup did not end')
                        raise RuntimeError('resume mismatch; owned Worker terminated for cleanup')
                    released += 1
            return released
        finally:
            for handle, _ in self.threads.values():
                base.close(handle)
            self.threads.clear()


def emit(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('pid', type=int)
    parser.add_argument('root')
    parser.add_argument('digest')
    parser.add_argument('--hold-ms', type=int, default=5000)
    args = parser.parse_args()
    if not 1 <= args.hold_ms <= 10000:
        raise ValueError('finite hold bound required')
    print('hold setup: verifying Worker identity', file=sys.stderr, flush=True)
    process, created = checked_worker(args.pid, args.root, args.digest)
    held = None
    finished, finalization = threading.Event(), threading.Lock()

    def watchdog():
        # A stuck suspension/context API must not leave an owned process frozen.
        # This exceptional cleanup is NEVER successful Host cancellation:88 is
        # a mandatory failure. The handle cannot be closed/reused under this lock.
        if not finished.wait(args.hold_ms / 1000 + 5):
            with finalization:
                if not finished.is_set():
                    base.kill(process, 87)
                    base.wait(process, 5000)
                    os._exit(88)

    threading.Thread(target=watchdog, daemon=True).start()
    try:
        print('hold setup: confirming suspended Worker threads', file=sys.stderr, flush=True)
        held = HeldThreads(process)
        count = len(held.threads)
        emit({'event': 'held', 'pid': args.pid, 'creationFileTime': created, 'threads': count})
        stdin = msvcrt.get_osfhandle(sys.stdin.fileno())
        deadline = time.monotonic() + args.hold_ms / 1000
        while alive(process):
            available = w.DWORD()
            if not peek_pipe(stdin, None, 0, None, c.byref(available), None):
                error = c.get_last_error()
                if error not in (109, 232):  # Broken/closing controller pipe.
                    raise c.WinError(error)
                reason = 'eof-or-release'
                break
            if available.value:
                reason = 'eof-or-release'
                break
            if time.monotonic() >= deadline:
                reason = 'timeout'
                break
            time.sleep(.01)
        else:
            reason = 'original-exit'
        exited = not alive(process)
        released = held.release()
        emit({'event': 'released', 'reason': reason, 'originalHandleSignalled': exited,
              'threads': count, 'resumed': released, 'forcedCleanup': held.forced_cleanup})
    finally:
        try:
            if held:
                held.release()
        finally:
            with finalization:
                finished.set()
                base.close(process)  # Release dead-token references before Host reclamation.


if __name__ == '__main__':
    main()
