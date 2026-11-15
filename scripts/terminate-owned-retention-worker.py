"""Fault only an exact benchmark-owned Worker, through its checked live handle."""
import ctypes as c
from ctypes import wintypes as w
import hashlib
from pathlib import Path
import sys

pid, root, digest = sys.argv[1:]
assert pid.isdecimal() and int(pid) > 0
root = Path(root).resolve(strict=True)
assert root.name == 'workers' and root.parent.name.startswith('diyime-primitive-retention-')
assert len(digest) == 64 and all(x in '0123456789abcdef' for x in digest)
kernel = c.WinDLL('kernel32', use_last_error=True)
def api(name, result, *args):
    fn = getattr(kernel, name)
    fn.restype, fn.argtypes = result, args
    return fn
open_process = api('OpenProcess', w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
image_name = api('QueryFullProcessImageNameW', w.BOOL, w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD))
get_exit = api('GetExitCodeProcess', w.BOOL, w.HANDLE, c.POINTER(w.DWORD))
terminate = api('TerminateProcess', w.BOOL, w.HANDLE, w.UINT)
wait = api('WaitForSingleObject', w.DWORD, w.HANDLE, w.DWORD)
close = api('CloseHandle', w.BOOL, w.HANDLE)
handle = open_process(0x1000 | 0x100000 | 0x1, False, int(pid))
if not handle:
    raise c.WinError(c.get_last_error())
try:
    buffer, size = c.create_unicode_buffer(32768), w.DWORD(32768)
    assert image_name(handle, 0, buffer, c.byref(size))
    path = Path(buffer.value).resolve(strict=True)
    assert root in path.parents and path.name == 'worker.exe'
    assert path.parent.name.startswith('DIYIME2.Worker.')
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
    status = w.DWORD()
    assert get_exit(handle, c.byref(status)) and status.value == 259
    assert terminate(handle, 86)
    assert wait(handle, 5000) == 0
    assert get_exit(handle, c.byref(status)) and status.value == 86
finally:
    close(handle)
print('Terminated exact benchmark Worker; original checked handle signalled')
