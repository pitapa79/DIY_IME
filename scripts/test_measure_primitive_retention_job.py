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
            self.assertEqual((access, inherit), (sampler.QUERY_PROCESS_AND_WAIT, False))
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

        expected_events = [('query',)]
        for pid in expected_order:
            expected_events.append(('open', pid))
            if pid != 11:
                expected_events.extend([('read', pid + 100), ('close', pid + 100)])
                if fail_read:
                    break
        with patch.object(sampler.base, 'query', side_effect=query), \
             patch.object(sampler, 'open_process', side_effect=open_pid), \
             patch.object(sampler, 'read_live_member_memory', side_effect=read), \
             patch.object(sampler.base, 'close', side_effect=lambda handle: events.append(('close', handle))):
            if fail_read:
                with self.assertRaisesRegex(RuntimeError, 'synthetic counter failure'):
                    sampler.sample(99, 0, previous_pids)
                self.assertEqual(events, expected_events)
            else:
                rows = sampler.sample(99, 0, previous_pids)['processes']
                self.assertEqual([row['pid'] for row in rows], [10, 11, 12])
                expected = {'pid': 11, 'available': False, 'error': 87, 'failureApi': 'OpenProcess'}
                if after_ids is not None:
                    expected['listedInJobAfterSample'] = 11 in after_ids
                self.assertEqual(rows[1], expected)
                self.assertEqual([row['available'] for row in rows], [True, False, True])
                self.assertEqual(events, expected_events + [('query',)])

    def test_read_before_next_open_and_preserve_missing(self):
        self.run_sample(set(), [10, 11, 12])

    def test_open_new_pids_first_and_preserve_enumeration_order_in_report(self):
        self.run_sample({10, 9}, [11, 12, 10])

    def test_close_current_handle_and_stop_if_a_read_raises(self):
        self.run_sample({10, 11}, [12, 10, 11], fail_read=True)

    def test_report_a_missing_pid_still_listed_in_the_job(self):
        self.run_sample(set(), [10, 11, 12], after_ids=(10, 11, 12))

    def test_preserve_missing_memory_if_the_second_job_query_fails(self):
        self.run_sample(set(), [10, 11, 12], after_ids=None)


class MemorySampleLivenessTests(unittest.TestCase):
    def test_only_publish_counters_read_while_process_is_unsignalled(self):
        def read(_handle, _job, observation):
            observation.update(available=True, workingSetBytes=4096)

        live = sampler.WAIT_TIMEOUT
        for waits, phase in [([live, live], None), ([0], 'before-read'), ([live, 0], 'after-read')]:
            with self.subTest(phase=phase):
                row = {'pid': 10, 'available': False}
                with patch.object(sampler.base, 'wait', side_effect=waits) as wait, \
                     patch.object(sampler, 'read_member_memory', side_effect=read) as counter:
                    sampler.read_live_member_memory(110, 99, row)
                self.assertEqual(wait.call_count, len(waits))
                self.assertEqual(counter.call_count, int(phase != 'before-read'))
                if phase is None:
                    self.assertEqual(row, {'pid': 10, 'available': True, 'workingSetBytes': 4096})
                else:
                    self.assertEqual(row, {'pid': 10, 'available': False, 'phase': phase,
                                           'failureApi': 'WaitForSingleObject', 'waitStatus': 0})


if __name__ == '__main__':
    unittest.main()
