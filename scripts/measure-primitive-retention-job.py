"""Job CPU/commit and sampled per-process memory, preserving failed runs too.

Working sets include shared pages: their sum is not unique physical RAM. Reads
that race process exit or fail are explicit, never filled with zero.
"""
import ctypes as c
from ctypes import wintypes as w
from contextlib import suppress
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import threading
import time

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location('retention_job_base', Path(__file__).with_name('measure-windows-benchmark-job.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

class Pids(c.Structure):
    _fields_ = [('assigned', w.DWORD), ('listed', w.DWORD), ('ids', c.c_size_t * 64)]

class Memory(c.Structure):
    _fields_ = [('cb', w.DWORD), ('faults', w.DWORD)] + [(name, c.c_size_t) for name in [
        'peak_ws', 'ws', 'peak_paged', 'paged', 'peak_nonpaged', 'nonpaged',
        'commit', 'peak_commit', 'private']]

open_process = base.api('OpenProcess', w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
in_job = base.api('IsProcessInJob', w.BOOL, w.HANDLE, w.HANDLE, c.POINTER(w.BOOL))
memory_info = base.api('K32GetProcessMemoryInfo', w.BOOL, w.HANDLE, c.POINTER(Memory), w.DWORD)
process_times = base.api('GetProcessTimes', w.BOOL, w.HANDLE, *([c.POINTER(w.FILETIME)] * 4))
create_port = base.api('CreateIoCompletionPort', w.HANDLE, w.HANDLE, w.HANDLE, c.c_size_t, w.DWORD)
get_packet = base.api('GetQueuedCompletionStatus', w.BOOL, w.HANDLE, c.POINTER(w.DWORD),
                      c.POINTER(c.c_size_t), c.POINTER(c.c_void_p), w.DWORD)
post_packet = base.api('PostQueuedCompletionStatus', w.BOOL, w.HANDLE, w.DWORD, c.c_size_t, c.c_void_p)

QUERY_PROCESS_AND_WAIT = 0x101000  # PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE
WAIT_TIMEOUT = 258

def read_member_memory(handle, job, row):
    member = w.BOOL()
    counters = Memory()
    counters.cb = c.sizeof(counters)
    created, exited, kernel, user = (w.FILETIME() for _ in range(4))
    if not in_job(handle, job, c.byref(member)):
        row['win32Error'] = c.get_last_error()
        row['error'] = 'not-current-job-member'
        row['failureApi'] = 'IsProcessInJob'
    elif not member.value:
        row['error'] = 'not-current-job-member'
    elif not memory_info(handle, c.byref(counters), c.sizeof(counters)):
        row['error'] = c.get_last_error()
        row['failureApi'] = 'K32GetProcessMemoryInfo'
    elif not process_times(handle, c.byref(created), c.byref(exited), c.byref(kernel), c.byref(user)):
        row['error'] = c.get_last_error()
        row['failureApi'] = 'GetProcessTimes'
    else:
        row.update(available=True, creationFileTime=str((created.dwHighDateTime << 32) | created.dwLowDateTime),
                   workingSetBytes=counters.ws, peakWorkingSetBytes=counters.peak_ws,
                   privateCommitBytes=counters.private)


def read_live_member_memory(handle, job, row):
    before = base.wait(handle, 0)
    if before != WAIT_TIMEOUT:
        row.update(failureApi='WaitForSingleObject', waitStatus=before, phase='before-read')
        return
    observation = dict(row)
    read_member_memory(handle, job, observation)
    after = base.wait(handle, 0)
    if after != WAIT_TIMEOUT:
        row.update(failureApi='WaitForSingleObject', waitStatus=after, phase='after-read')
        return
    row.update(observation)


class CreationMemoryObserver:
    """Collect supplemental live reads from best-effort Job notifications."""

    class Association(c.Structure):
        _fields_ = [('key', c.c_void_p), ('port', w.HANDLE)]

    def __init__(self, job, origin):
        self.job = job
        self.origin = origin
        self.rows = []
        self.dropped = 0
        self.error = None
        self.stop = threading.Event()
        self.port = base.check(create_port(c.c_void_p(-1), None, 0, 1))
        try:
            association = self.Association(1, self.port)
            base.check(base.set_job(job, 7, c.byref(association), c.sizeof(association)))
            self.thread = threading.Thread(target=self.listen, name='job-creation-memory')
            self.thread.start()
        except BaseException:
            base.close(self.port)
            raise

    def listen(self):
        try:
            message, key, pid = w.DWORD(), c.c_size_t(), c.c_void_p()
            while not self.stop.is_set():
                if not get_packet(self.port, c.byref(message), c.byref(key), c.byref(pid), 100):
                    if c.get_last_error() != WAIT_TIMEOUT:
                        raise c.WinError(c.get_last_error())
                    continue
                if key.value == 0:  # Stop packet queued by close().
                    return
                if key.value == 1 and message.value == 6 and pid.value:  # JOB_OBJECT_MSG_NEW_PROCESS
                    if len(self.rows) == 4096:
                        self.dropped += 1
                    else:
                        self.rows.append(self.observe(pid.value))
        except Exception as error:
            self.error = str(error)

    def observe(self, pid):
        row = dict(pid=pid, available=False, elapsedMs=(time.monotonic()-self.origin)*1000)
        handle = open_process(QUERY_PROCESS_AND_WAIT, False, pid)
        if not handle:
            row.update(failureApi='OpenProcess', error=c.get_last_error())
            return row
        try:
            read_live_member_memory(handle, self.job, row)
            return row
        finally:
            base.close(handle)

    def close(self):
        if self.port is not None:
            if not post_packet(self.port, 0, 0, None):
                self.error = f'PostQueuedCompletionStatus failed: {c.get_last_error()}'
                self.stop.set()
            self.thread.join()
            base.close(self.port)
            self.port = None


def sample(job, origin, previous_pids):
    pids = base.query(job, 3, Pids())
    if pids.assigned > 64 or pids.listed > 64:
        raise RuntimeError('bounded memory sampler PID capacity exceeded')
    rows = [dict(pid=pid, available=False) for pid in pids.ids[:pids.listed]]
    # Visit newly listed processes first while keeping the Job's original
    # enumeration order in the report. Read before opening the next handle.
    for row in sorted(rows, key=lambda row: row['pid'] in previous_pids):
        handle = open_process(QUERY_PROCESS_AND_WAIT, False, row['pid'])
        if not handle:
            row.update(error=c.get_last_error(), failureApi='OpenProcess')
            continue
        try:
            read_live_member_memory(handle, job, row)
        finally:
            base.close(handle)
    elapsed_ms = (time.monotonic()-origin)*1000
    missing = [row for row in rows if not row['available']]
    if missing:
        # Recheck after closing the observation handles. A PID absent now does
        # not repair its missed sample or establish the time of process exit.
        after = None
        try:
            after = base.query(job, 3, Pids())
        except OSError:
            pass
        if after is not None and after.assigned == after.listed <= 64:
            listed = set(after.ids[:after.listed])
            for row in missing:
                row['listedInJobAfterSample'] = row['pid'] in listed
    return dict(elapsedMs=elapsed_ms, assigned=pids.assigned,
                listed=pids.listed, processes=rows)


def process_v2_observer():
    spec = importlib.util.spec_from_file_location(
        'process_v2_job_memory', Path(__file__).with_name('process_v2_job_memory.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    def members(job):
        pids = base.query(job, 3, Pids())
        if pids.assigned == pids.listed <= len(pids.ids):
            return set(pids.ids[:pids.listed])
        return None

    return module.ProcessV2MemoryObserver(members)


def measure(command, observe_creation=False, observe_process_v2=False):
    job = base.check(base.create_job(None, None))
    process = base.Process()
    assigned = False
    origin = time.monotonic()
    samples = []
    previous_pids = set()
    timed_out = False
    creation = None
    process_v2 = None
    process_v2_rows = []
    process_v2_error = None
    try:
        limits = base.ExtendedLimits()
        limits.basic.flags = 0x2000
        base.check(base.set_job(job, 9, c.byref(limits), c.sizeof(limits)))
        if observe_creation:
            creation = CreationMemoryObserver(job, origin)
        if observe_process_v2:
            try:
                process_v2 = process_v2_observer()
            except OSError as error:
                process_v2_error = str(error)
        startup = base.Startup()
        startup.size = c.sizeof(startup)
        line = c.create_unicode_buffer(subprocess.list2cmdline(command))
        base.check(base.create(command[0], line, None, None, False, 4, None, None, c.byref(startup), c.byref(process)))
        base.check(base.assign(job, process.handle))
        assigned = True
        if base.resume(process.thread) == 0xffffffff:
            raise c.WinError(c.get_last_error())
        while True:
            result = base.wait(process.handle, 100)
            if process_v2 is not None:
                try:
                    extra = process_v2.sample(job)
                except (OSError, ValueError) as error:
                    extra = {'error': str(error)}
                process_v2_rows.append(dict(primarySampleIndex=len(samples), **extra))
            observation = sample(job, origin, previous_pids)
            samples.append(observation)
            previous_pids = {row['pid'] for row in observation['processes']}
            if result == 0:
                break
            if result != WAIT_TIMEOUT:
                raise RuntimeError('original benchmark driver wait failed')
            if time.monotonic()-origin >= 300:
                timed_out = True
                break
        status = w.DWORD()
        base.check(base.exit_code(process.handle, c.byref(status)))
        accounting = base.query(job, 1, base.Accounting())
        until = time.monotonic()+10
        while accounting.active and not timed_out and time.monotonic() < until:
            time.sleep(.01)
            accounting = base.query(job, 1, base.Accounting())
        missing = sum(not row['available'] for s in samples for row in s['processes'])
        result = dict(cpuMs=(accounting.user+accounting.kernel)/10_000,
                    peakJobCommitBytes=base.query(job, 9, base.ExtendedLimits()).peak_job,
                    ownedProcessCount=accounting.processes, activeAtObservation=accounting.active,
                    driverExitCode=status.value, timedOut=timed_out,
                    allDescendantsExited=accounting.active == 0,
                    successful=not timed_out and status.value == 0 and accounting.active == 0,
                    memorySamplingIntervalMs=100, missingProcessMemoryReads=missing,
                    workingSetSumCountsSharedPagesMultipleTimes=True,
                    samples=samples)
        if creation is not None:
            creation.close()
            result['creationMemoryObservations'] = dict(rows=creation.rows, dropped=creation.dropped,
                                                        listenerError=creation.error)
        if observe_process_v2:
            if process_v2 is not None:
                try:
                    process_v2.close()
                except OSError as error:
                    process_v2_error = str(error)
            result['processV2MemoryObservations'] = dict(rows=process_v2_rows, observerError=process_v2_error)
        return result
    finally:
        if creation is not None:
            creation.close()
        if process_v2 is not None:
            with suppress(OSError):
                process_v2.close()
        if process.handle:
            if not assigned:
                base.kill(process.handle, 1)
            base.close(process.thread)
            base.close(process.handle)
        base.close(job)

def main():
    args = sys.argv[1:]
    options = set()
    while args[0] in ('--observe-creation-memory', '--observe-process-v2-memory'):
        options.add(args[0])
        args = args[1:]
    assert args[0] == '--command'
    report = Path(args[1]).resolve()
    assert not report.exists()
    command = [str(Path(args[2]).resolve(strict=True)), *args[3:]]
    result = measure(command, observe_creation='--observe-creation-memory' in options,
                     observe_process_v2='--observe-process-v2-memory' in options)
    with report.open('x', encoding='utf-8') as stream:
        json.dump(result, stream)
    print(json.dumps({k:v for k,v in result.items() if k not in
                      {'samples', 'creationMemoryObservations', 'processV2MemoryObservations'}}), file=sys.stderr)
    if not result['successful']:
        raise SystemExit(124 if result['timedOut'] else result['driverExitCode'] or 1)


if __name__ == '__main__':
    main()
