"""Actual Windows test-only deletion refusal on one exact new fixture file."""
import ctypes as c
from ctypes import wintypes as w
import json,os,sys,tempfile,threading,time
from pathlib import Path

path=Path(sys.argv[1]).resolve(strict=True)
parent=Path(tempfile.gettempdir()).resolve(strict=True)
if path.name!='locked.txt' or path.parent.parent!=parent or not path.parent.name.startswith('diyime-app-smoke-file-lock-'):
    raise ValueError('exact private cleanup fixture required')
mode=sys.argv[2]
if mode not in ('transient','persistent'):raise ValueError('finite fixture mode required')
kernel=c.WinDLL('kernel32',use_last_error=True)
create=kernel.CreateFileW
create.argtypes=[w.LPCWSTR,w.DWORD,w.DWORD,c.c_void_p,w.DWORD,w.DWORD,w.HANDLE]
create.restype=w.HANDLE
close=kernel.CloseHandle
close.argtypes=[w.HANDLE]
close.restype=w.BOOL
# Share read/write, deliberately deny FILE_SHARE_DELETE on this test-owned file.
handle=create(str(path),0x80000000,3,None,3,0x80,None)
if not handle or handle==c.c_void_p(-1).value:raise c.WinError(c.get_last_error())
watchdog=threading.Timer(20,lambda:os._exit(91))
watchdog.daemon=True
watchdog.start()
try:
    print(json.dumps({'event':'held'}),flush=True)
    signal=sys.stdin.buffer.read(1)
    if mode=='transient':
        if signal!=b'r':raise ValueError('explicit release-delay signal required')
        time.sleep(.2)
    elif signal:
        raise ValueError('persistent fixture requires controller EOF')
finally:
    if not close(handle):raise c.WinError(c.get_last_error())
    watchdog.cancel()
print(json.dumps({'event':'released'}),flush=True)
