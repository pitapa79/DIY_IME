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
    def run_sample(self, previous_pids, expected_order, fail_read=False, after_ids=(10, 12)):
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
        def query(job, kind, _):
            self.assertEqual((job, kind), (99, 3))
            events.append(('query',))
            if events == [('query',)]:
                return pids
            if after_ids is None:
                raise OSError('synthetic Job query failure')
            return SimpleNamespace(assigned=len(after_ids), listed=len(after_ids), ids=after_ids)

        open_events = [('open', pid) for pid in expected_order]
        opened = [pid + 100 for pid in expected_order if pid != 11]
        close_events = [('close', handle) for handle in reversed(opened)]
        with patch.object(sampler.base, 'query', side_effect=query), \
             patch.object(sampler, 'open_process', side_effect=open_pid), \
             patch.object(sampler, 'read_member_memory', side_effect=read), \
             patch.object(sampler.base, 'close', side_effect=lambda handle: events.append(('close', handle))):
            if fail_read:
                with self.assertRaisesRegex(RuntimeError, 'synthetic counter failure'):
                    sampler.sample(99, 0, previous_pids)
                self.assertEqual(events, [('query',)] + open_events + [('read', opened[0])] + close_events)
            else:
                rows = sampler.sample(99, 0, previous_pids)['processes']
                self.assertEqual([row['pid'] for row in rows], [10, 11, 12])
                expected = {'pid': 11, 'available': False, 'error': 87, 'failureApi': 'OpenProcess'}
                if after_ids is not None:
                    expected['listedInJobAfterSample'] = 11 in after_ids
                self.assertEqual(rows[1], expected)
                self.assertEqual([row['available'] for row in rows], [True, False, True])
                self.assertEqual(events, [('query',)] + open_events + [('read', handle) for handle in opened]
                                 + close_events + [('query',)])

    def test_bind_all_pids_before_reading_memory_and_preserve_missing(self):
        self.run_sample(set(), [10, 11, 12])

    def test_open_new_pids_first_and_preserve_enumeration_order_in_report(self):
        self.run_sample({10, 9}, [11, 12, 10])

    def test_close_every_open_handle_if_a_read_raises(self):
        self.run_sample({10, 11}, [12, 10, 11], fail_read=True)

    def test_report_a_missing_pid_still_listed_in_the_job(self):
        self.run_sample(set(), [10, 11, 12], after_ids=(10, 11, 12))

    def test_preserve_missing_memory_if_the_second_job_query_fails(self):
        self.run_sample(set(), [10, 11, 12], after_ids=None)


if __name__ == '__main__':
    unittest.main()
