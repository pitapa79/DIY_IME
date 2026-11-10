"""Local Windows Job accounting for the driver and every benchmark descendant."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess
import sys
import time

kernel = c.WinDLL("kernel32", use_last_error=True)
size = c.c_size_t
u64 = c.c_uint64
i64 = c.c_int64


class BasicLimits(c.Structure):
    _fields_ = [("process_time", i64), ("job_time", i64), ("flags", w.DWORD),
                ("min_ws", size), ("max_ws", size), ("active_limit", w.DWORD),
                ("affinity", size), ("priority", w.DWORD), ("scheduling", w.DWORD)]


class ExtendedLimits(c.Structure):
    _fields_ = [("basic", BasicLimits), ("io", u64 * 6),
                ("process_memory", size), ("job_memory", size),
                ("peak_process", size), ("peak_job", size)]


class Accounting(c.Structure):
    _fields_ = [("user", i64), ("kernel", i64), ("period_user", i64), ("period_kernel", i64),
                ("faults", w.DWORD), ("processes", w.DWORD), ("active", w.DWORD), ("terminated", w.DWORD)]


class Startup(c.Structure):
    _fields_ = [("size", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
                ("title", w.LPWSTR), ("coordinates", w.DWORD * 8), ("show", w.WORD),
                ("reserved_bytes", w.WORD), ("reserved_pointer", c.c_void_p),
                ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE)]


class Process(c.Structure):
    _fields_ = [("handle", w.HANDLE), ("thread", w.HANDLE), ("pid", w.DWORD), ("tid", w.DWORD)]


def api(name, result, *args):
    fn = getattr(kernel, name)
    fn.restype = result
    fn.argtypes = args
    return fn


create_job = api("CreateJobObjectW", w.HANDLE, c.c_void_p, w.LPCWSTR)
set_job = api("SetInformationJobObject", w.BOOL, w.HANDLE, c.c_int, c.c_void_p, w.DWORD)
query_job = api("QueryInformationJobObject", w.BOOL, w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p)
create = api("CreateProcessW", w.BOOL, w.LPCWSTR, w.LPWSTR, c.c_void_p, c.c_void_p, w.BOOL,
             w.DWORD, c.c_void_p, w.LPCWSTR, c.POINTER(Startup), c.POINTER(Process))
assign = api("AssignProcessToJobObject", w.BOOL, w.HANDLE, w.HANDLE)
resume = api("ResumeThread", w.DWORD, w.HANDLE)
wait = api("WaitForSingleObject", w.DWORD, w.HANDLE, w.DWORD)
exit_code = api("GetExitCodeProcess", w.BOOL, w.HANDLE, c.POINTER(w.DWORD))
kill = api("TerminateProcess", w.BOOL, w.HANDLE, w.UINT)
close = api("CloseHandle", w.BOOL, w.HANDLE)


def check(value):
    if not value:
        raise c.WinError(c.get_last_error())
    return value


def query(job, cls, value):
    check(query_job(job, cls, c.byref(value), c.sizeof(value), None))
    return value


def measure(command, timeout_ms):
    job = check(create_job(None, None))
    process = Process()
    assigned = False
    try:
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # KILL_ON_JOB_CLOSE
        check(set_job(job, 9, c.byref(limits), c.sizeof(limits)))
        startup = Startup()
        startup.size = c.sizeof(startup)
        line = c.create_unicode_buffer(subprocess.list2cmdline(command))
        # Attach before any driver code can create a descendant.
        check(create(command[0], line, None, None, False, 4, None, None,
                     c.byref(startup), c.byref(process)))
        check(assign(job, process.handle))
        assigned = True
        if resume(process.thread) == 0xffffffff:
            raise c.WinError(c.get_last_error())
        if wait(process.handle, timeout_ms) != 0:
            raise RuntimeError("benchmark driver did not finish")
        status = w.DWORD()
        check(exit_code(process.handle, c.byref(status)))
        if status.value:
            raise RuntimeError("benchmark driver exited " + str(status.value))
        until = time.monotonic() + 10
        accounting = query(job, 1, Accounting())
        while accounting.active:
            if time.monotonic() >= until:
                raise RuntimeError("benchmark left live descendants")
            time.sleep(.01)
            accounting = query(job, 1, Accounting())
        peak = query(job, 9, ExtendedLimits()).peak_job
        return {"cpuMs": (accounting.user + accounting.kernel) / 10_000,
                "peakJobCommitBytes": peak, "ownedProcessCount": accounting.processes}
    finally:
        if process.handle:
            if not assigned:
                kill(process.handle, 1)
            close(process.thread)
            close(process.handle)
        close(job)


def main(args):
    arbitrary_command = args[0] == "--command"
    if arbitrary_command:
        report = Path(args[1]).resolve()
        command = [str(Path(args[2]).resolve(strict=True)), *args[3:]]
        timeout = 300_000
    else:
        bundle, benchmark_report = map(Path, args[:2])
        rounds = int(args[2])
        if not 1 <= rounds <= 10:
            raise ValueError("expected 1–10 rounds")
        bundle, benchmark_report = bundle.resolve(strict=True), benchmark_report.resolve()
        report = benchmark_report.with_suffix(".job.json")
        driver = bundle / "runtime" / "node.exe"
        js = """import {benchmark} from './scripts/benchmark-windows-application.mjs';
import {writeFileSync} from 'node:fs';
const [folder,output,rounds]=process.argv.slice(1);
const report=await benchmark({folder,hostInputRounds:Number(rounds),pollMs:5,trace:0});
writeFileSync(output,JSON.stringify(report),{flag:'wx'});"""
        command = [str(driver), "--input-type=module", "-e", js, str(bundle),
                   str(benchmark_report), str(rounds)]
        timeout = 300_000 + rounds * 240_000
    if report.exists():
        raise FileExistsError(report)
    result = measure(command, timeout)
    with report.open("x", encoding="utf-8") as f:
        json.dump(result, f)
    print(json.dumps(result), file=sys.stderr if arbitrary_command else sys.stdout)


if __name__ == "__main__":
    main(sys.argv[1:])
