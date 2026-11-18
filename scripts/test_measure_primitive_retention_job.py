"""Focused regression for the Windows Job sampler's handle ordering."""
import ctypes
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch


spec = spec_from_file_location('sampler_under_test', Path(__file__).with_name('measure-primitive-retention-job.py'))
sampler = module_from_spec(spec)
spec.loader.exec_module(sampler)


class MemorySampleHandleTests(unittest.TestCase):
    def run_sample(self, fail_read=False):
        events = []

        def open_pid(access, inherit, pid):
            self.assertEqual((access, inherit), (0x1000, False))
            events.append(('open', pid))
            if pid == 11:
                ctypes.set_last_error(87)
                return None
            return pid + 100

        def read(handle, job, row):
            self.assertEqual(job, 99)
            events.append(('read', handle))
            if fail_read:
                raise RuntimeError('synthetic counter failure')
            row['available'] = True

        pids = SimpleNamespace(assigned=3, listed=3, ids=[10, 11, 12])
        with patch.object(sampler.base, 'query', return_value=pids), \
             patch.object(sampler, 'open_process', side_effect=open_pid), \
             patch.object(sampler, 'read_member_memory', side_effect=read), \
             patch.object(sampler.base, 'close', side_effect=lambda handle: events.append(('close', handle))):
            if fail_read:
                with self.assertRaisesRegex(RuntimeError, 'synthetic counter failure'):
                    sampler.sample(99, 0)
                self.assertEqual(events, [('open', 10), ('open', 11), ('open', 12), ('read', 110),
                                          ('close', 112), ('close', 110)])
            else:
                rows = sampler.sample(99, 0)['processes']
                self.assertEqual(rows[1], {'pid': 11, 'available': False, 'error': 87, 'failureApi': 'OpenProcess'})
                self.assertEqual([row['available'] for row in rows], [True, False, True])
                self.assertEqual(events, [('open', 10), ('open', 11), ('open', 12), ('read', 110),
                                          ('read', 112), ('close', 112), ('close', 110)])

    def test_bind_all_pids_before_reading_memory_and_preserve_missing(self):
        self.run_sample()

    def test_close_every_open_handle_if_a_read_raises(self):
        self.run_sample(fail_read=True)


if __name__ == '__main__':
    unittest.main()
