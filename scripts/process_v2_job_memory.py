"""Supplemental Process V2 samples bracketed by two Windows Job PID lists."""

import ctypes as c
from ctypes import wintypes as w
import time


class RawCounter(c.Structure):
    _fields_ = [('status', w.DWORD), ('timestamp', w.FILETIME),
                ('first', c.c_int64), ('second', c.c_int64), ('multiple', w.DWORD)]


class CounterItem(c.Structure):
    _fields_ = [('instance', w.LPWSTR), ('raw', RawCounter)]


class ProcessV2MemoryObserver:
    COUNTERS = {'Process ID': 'pid', 'Working Set': 'workingSetBytes',
                'Working Set Peak': 'peakWorkingSetBytes', 'Private Bytes': 'privateCommitBytes',
                'Elapsed Time': 'elapsedTimeStartCounter'}
    PDH_MORE_DATA = 0x800007d2

    def __init__(self, members):
        self.members = members
        self.query = w.HANDLE()
        pdh = c.WinDLL('pdh')

        def api(name, *arguments):
            fn = getattr(pdh, name)
            fn.argtypes, fn.restype = arguments, w.DWORD
            return fn

        pointer = c.POINTER
        open_query = api('PdhOpenQueryW', w.LPCWSTR, c.c_size_t, pointer(w.HANDLE))
        add = api('PdhAddCounterW', w.HANDLE, w.LPCWSTR, c.c_size_t, pointer(w.HANDLE))
        self.collect = api('PdhCollectQueryDataWithTime', w.HANDLE, pointer(c.c_int64))
        self.raw_array = api('PdhGetRawCounterArrayW', w.HANDLE, pointer(w.DWORD), pointer(w.DWORD), c.c_void_p)
        self.close_query = api('PdhCloseQuery', w.HANDLE)
        self.check(open_query(None, 0, c.byref(self.query)))
        try:
            self.counters = {}
            for label, field in self.COUNTERS.items():
                counter = w.HANDLE()
                self.check(add(self.query, '\\Process V2(*)\\' + label, 0, c.byref(counter)))
                self.counters[field] = counter
        except BaseException:
            self.close()
            raise

    @staticmethod
    def check(status):
        if status:
            raise OSError(f'PDH status 0x{status:08x}')

    def values(self, counter):
        size, count = w.DWORD(), w.DWORD()
        status = self.raw_array(counter, c.byref(size), c.byref(count), None)
        if status != self.PDH_MORE_DATA or not 0 < size.value <= 1024 * 1024:
            raise OSError(f'PDH array size unavailable: status=0x{status:08x}, size={size.value}')
        buffer = c.create_string_buffer(size.value)
        self.check(self.raw_array(counter, c.byref(size), c.byref(count), buffer))
        if count.value * c.sizeof(CounterItem) > c.sizeof(buffer):
            raise ValueError('PDH returned an invalid counter array length')
        return [(row.instance, row.raw.first) for row in c.cast(buffer, c.POINTER(CounterItem))[:count.value]
                if row.instance and row.raw.status in (0, 1) and row.raw.first >= 0]

    def sample(self, job):
        start = time.perf_counter_ns()
        before = self.members(job)
        stamp = c.c_int64()
        self.check(self.collect(self.query, c.byref(stamp)))
        after = self.members(job)
        if before is None or after is None:
            raise ValueError('Job PID enumeration changed while being read')
        shared = before & after
        owned = [(instance, pid) for instance, pid in self.values(self.counters['pid']) if pid in shared]
        by_instance = dict(owned)
        if len(by_instance) != len(owned) or len(set(by_instance.values())) != len(owned):
            raise ValueError('PDH returned ambiguous instances for a Job PID')
        counters = {pid: {'pid': pid} for pid in by_instance.values()}
        for field, counter in self.counters.items():
            if field == 'pid':
                continue
            for instance, value in self.values(counter):
                if instance in by_instance:
                    row = counters[by_instance[instance]]
                    if field in row:
                        raise ValueError('PDH returned duplicate values for a Job PID')
                    row[field] = value
        return dict(sampleFileTime=str(stamp.value), observationMs=(time.perf_counter_ns() - start) / 1e6,
                    listedBefore=sorted(before), listedAfter=sorted(after),
                    processes=[row for row in counters.values() if row.keys() == self.counters.keys()])

    def close(self):
        if self.query:
            query, self.query = self.query, None
            self.check(self.close_query(query))
