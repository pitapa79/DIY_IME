"""Test-only original launcher handle and outer Job; no PID ancestry authority.

The Job stays open through assertEmpty. Its KILL_ON_JOB_CLOSE is cleanup only,
never evidence that the product itself ended its descendants.
"""
import ctypes as c
from ctypes import wintypes as w
import importlib.util
import json
import msvcrt
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading

spec = importlib.util.spec_from_file_location(
    'owned_job_base', Path(__file__).with_name('measure-windows-benchmark-job.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

image_name = base.api('QueryFullProcessImageNameW', w.BOOL, w.HANDLE, w.DWORD,
                      w.LPWSTR, c.POINTER(w.DWORD))
open_process = base.api('OpenProcess', w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
is_in_job = base.api('IsProcessInJob', w.BOOL, w.HANDLE, w.HANDLE, c.POINTER(w.BOOL))


def emit(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)


class LauncherJob:
    def __init__(self, command):
        self.job = base.check(base.create_job(None, None))
        self.process = base.Process()
        self.assigned = False
        try:
            limits = base.ExtendedLimits()
            limits.basic.flags = 0x2000  # KILL_ON_JOB_CLOSE; no breakaway.
            base.check(base.set_job(self.job, 9, c.byref(limits), c.sizeof(limits)))
            read_fd, write_fd = os.pipe()
            try:
                os.set_inheritable(write_fd, True)
                with open(os.devnull, 'r+b', buffering=0) as null:
                    os.set_inheritable(null.fileno(), True)
                    startup = base.Startup()
                    startup.size = c.sizeof(startup)
                    startup.coordinates[7] = 0x100  # STARTF_USESTDHANDLES
                    startup.stdin = startup.stderr = msvcrt.get_osfhandle(null.fileno())
                    startup.stdout = msvcrt.get_osfhandle(write_fd)
                    environment = dict(os.environ, PATH='')
                    block = c.create_unicode_buffer('\0'.join(
                        k + '=' + v for k, v in sorted(environment.items(),
                                                       key=lambda kv: kv[0].casefold())) + '\0\0')
                    line = c.create_unicode_buffer(subprocess.list2cmdline(command))
                    base.check(base.create(command[0], line, None, None, True,
                                           4 | 0x400 | 0x08000000, block, None,
                                           c.byref(startup), c.byref(self.process)))
                    base.check(base.assign(self.job, self.process.handle))
                    self.assigned = True
                    if base.resume(self.process.thread) == 0xffffffff:
                        raise c.WinError(c.get_last_error())
            except BaseException:
                os.close(read_fd)
                raise
            finally:
                os.close(write_fd)
            self.output = os.fdopen(read_fd, 'rb', buffering=0)
        except BaseException:
            self.close()
            raise

    def first_line(self):
        result = queue.Queue(maxsize=1)

        def read():
            try:
                result.put(self.output.readline(65537))
            except BaseException as error:
                result.put(error)

        threading.Thread(target=read, daemon=True).start()
        line = result.get(timeout=30)
        if isinstance(line, BaseException):
            raise line
        if not line or len(line) > 65536 or not line.endswith(b'\n'):
            raise RuntimeError('missing or oversized launcher startup line')
        return json.loads(line)

    def members(self):
        # Fixed finite list. Overflow fails closed, never an empty membership.
        class Ids(c.Structure):
            _fields_ = [('assigned', w.DWORD), ('listed', w.DWORD),
                        ('ids', c.c_size_t * 4096)]

        ids = base.query(self.job, 3, Ids())
        if ids.listed > 4096 or ids.assigned > 4096:
            raise RuntimeError('owned Job membership exceeds observation bound')
        members = []
        for pid in ids.ids[:ids.listed]:
            handle = open_process(0x1000, False, pid)
            if not handle:
                # A concurrent exit can remove it. Accounting below still
                # prevents an omitted live process becoming empty evidence.
                continue
            try:
                belongs = w.BOOL()
                base.check(is_in_job(handle, self.job, c.byref(belongs)))
                if not belongs.value:
                    continue  # PID recycled after the Job snapshot.
                buffer, length = c.create_unicode_buffer(32768), w.DWORD(32768)
                base.check(image_name(handle, 0, buffer, c.byref(length)))
                members.append({'pid': pid, 'image': Path(buffer.value).name})
            finally:
                base.close(handle)
        accounting = base.query(self.job, 1, base.Accounting())
        return {'active': accounting.active, 'listed': ids.listed, 'members': members}

    def assert_empty(self):
        value = self.members()
        if value['active'] or value['listed'] or value['members']:
            raise RuntimeError('original launcher left owned Job members: ' + json.dumps(value))
        return {'emptyBeforeJobClose': True, **value}

    def crash(self):
        if base.wait(self.process.handle, 0) != 258:
            raise RuntimeError('original launcher exited before deliberate crash')
        base.check(base.kill(self.process.handle, 86))
        if base.wait(self.process.handle, 5000) != 0:
            raise RuntimeError('original launcher handle did not signal')
        status = w.DWORD()
        base.check(base.exit_code(self.process.handle, c.byref(status)))
        if status.value != 86:
            raise RuntimeError('unexpected original launcher exit')
        return {'originalHandleSignalled': True, 'exitCode': status.value}

    def close(self):
        if self.process.handle:
            if not self.assigned:
                base.kill(self.process.handle, 1)
            base.close(self.process.thread)
            base.close(self.process.handle)
            self.process.handle = None
        if self.job:
            base.close(self.job)  # Cleanup ONLY this owned Job, including on EOF/failure.
            self.job = None
        if hasattr(self, 'output'):
            self.output.close()


def main(command):
    if not command or not Path(command[0]).is_absolute():
        raise ValueError('absolute launcher executable required')
    owner = LauncherJob(command)
    try:
        emit({'event': 'started', 'pid': owner.process.pid, 'startup': owner.first_line()})
        while True:
            line = sys.stdin.buffer.readline(4097)
            if not line:
                break
            if len(line) > 4096 or not line.endswith(b'\n'):
                raise ValueError('invalid bounded wrapper request')
            request = json.loads(line)
            try:
                method = request['method']
                if method == 'members':
                    data = owner.members()
                elif method == 'crash':
                    data = owner.crash()
                elif method == 'assertEmpty':
                    data = owner.assert_empty()
                else:
                    raise ValueError('unsupported wrapper operation')
                emit({'id': request['id'], 'ok': True, 'data': data})
            except Exception as error:
                emit({'id': request.get('id'), 'ok': False, 'error': str(error)})
    finally:
        owner.close()


if __name__ == '__main__':
    main(sys.argv[1:])
