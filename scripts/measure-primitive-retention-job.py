"""Job CPU/commit and sampled per-process memory, preserving failed runs too.

Working sets include shared pages: their sum is not unique physical RAM. Reads
that race process exit or fail are explicit, never filled with zero.
"""
import ctypes as c
from ctypes import wintypes as w
from contextlib import ExitStack
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
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


def sample(job, origin):
    pids = base.query(job, 3, Pids())
    if pids.assigned > 64 or pids.listed > 64:
        raise RuntimeError('bounded memory sampler PID capacity exceeded')
    rows = []
    pending = []
    with ExitStack() as handles:
        # Bind every observed PID promptly. Memory queries on earlier handles
        # should not lengthen the interval before opening later PIDs.
        for pid in pids.ids[:pids.listed]:
            row = dict(pid=pid, available=False)
            handle = open_process(0x1000, False, pid)
            if handle:
                handles.callback(base.close, handle)
                pending.append((handle, row))
            else:
                row['error'] = c.get_last_error()
                row['failureApi'] = 'OpenProcess'
            rows.append(row)
        for handle, row in pending:
            read_member_memory(handle, job, row)
    return dict(elapsedMs=(time.monotonic()-origin)*1000, assigned=pids.assigned,
                listed=pids.listed, processes=rows)


def measure(command):
    job = base.check(base.create_job(None, None))
    process = base.Process()
    assigned = False
    origin = time.monotonic()
    samples = []
    timed_out = False
    try:
        limits = base.ExtendedLimits()
        limits.basic.flags = 0x2000
        base.check(base.set_job(job, 9, c.byref(limits), c.sizeof(limits)))
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
            samples.append(sample(job, origin))
            if result == 0:
                break
            if result != 0x102:
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
        return dict(cpuMs=(accounting.user+accounting.kernel)/10_000,
                    peakJobCommitBytes=base.query(job, 9, base.ExtendedLimits()).peak_job,
                    ownedProcessCount=accounting.processes, activeAtObservation=accounting.active,
                    driverExitCode=status.value, timedOut=timed_out,
                    allDescendantsExited=accounting.active == 0,
                    successful=not timed_out and status.value == 0 and accounting.active == 0,
                    memorySamplingIntervalMs=100, missingProcessMemoryReads=missing,
                    workingSetSumCountsSharedPagesMultipleTimes=True,
                    samples=samples)
    finally:
        if process.handle:
            if not assigned:
                base.kill(process.handle, 1)
            base.close(process.thread)
            base.close(process.handle)
        base.close(job)

def main():
    assert sys.argv[1] == '--command'
    report = Path(sys.argv[2]).resolve()
    assert not report.exists()
    command = [str(Path(sys.argv[3]).resolve(strict=True)), *sys.argv[4:]]
    result = measure(command)
    with report.open('x', encoding='utf-8') as stream:
        json.dump(result, stream)
    print(json.dumps({k:v for k,v in result.items() if k != 'samples'}), file=sys.stderr)
    if not result['successful']:
        raise SystemExit(124 if result['timedOut'] else result['driverExitCode'] or 1)


if __name__ == '__main__':
    main()
