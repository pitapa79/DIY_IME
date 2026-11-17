"""Actual Windows Job tests, including a live descendant that MUST fail proof."""
import ctypes as c
from ctypes import wintypes as w
import importlib.util
from pathlib import Path
import sys
import time
import unittest

spec = importlib.util.spec_from_file_location('launcher_owner', Path(__file__).with_name('owned-launcher-job.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class OwnershipTests(unittest.TestCase):
    def test_ended_member_image_race_is_kept_until_fresh_zero(self):
        owner = module.LauncherJob([sys.executable, '-u', '-c',
                                   'import json,time;print(json.dumps({"ready":True}),flush=True);time.sleep(30)'])
        original = module.image_name
        process_id = module.base.api('GetProcessId', w.DWORD, w.HANDLE)

        def exit_during_image_query(handle, flags, buffer, length):
            if process_id(handle) == owner.process.pid:
                owner.crash()
                c.set_last_error(5)
                return False
            return original(handle, flags, buffer, length)

        try:
            owner.first_line()
            module.image_name = exit_during_image_query
            observed = owner.members()
            self.assertGreaterEqual(observed['listed'], 1)
            self.assertTrue(any(m['pid'] == owner.process.pid and m.get('exited')
                                for m in observed['members']))
            self.assertTrue(owner.job)
        finally:
            module.image_name = original
            owner.close()

    def test_live_member_image_denial_cannot_prove_empty(self):
        owner = module.LauncherJob([sys.executable, '-u', '-c',
                                   'import json,time;print(json.dumps({"ready":True}),flush=True);time.sleep(30)'])
        original = module.image_name

        def deny_live_image(*args):
            c.set_last_error(5)
            return False

        try:
            owner.first_line()
            module.image_name = deny_live_image
            observed = owner.members()
            self.assertTrue(any(m['pid'] == owner.process.pid and
                                m.get('imageError') == 5 and not m['exited']
                                for m in observed['members']))
            with self.assertRaisesRegex(RuntimeError, 'left owned Job members'):
                owner.assert_empty()
            self.assertEqual(module.base.wait(owner.process.handle, 0), 258)
        finally:
            module.image_name = original
            owner.close()

    def test_original_handle_crash_and_empty_before_outer_close(self):
        owner = module.LauncherJob([sys.executable, '-u', '-c',
                                   'import json,time;print(json.dumps({"ready":True}),flush=True);time.sleep(30)'])
        try:
            self.assertEqual(owner.first_line(), {'ready': True})
            before = owner.members()
            self.assertGreaterEqual(before['active'], 1)
            self.assertIn(owner.process.pid, [m['pid'] for m in before['members']])
            # Windows may also own a conhost. It stays in the observation and
            # must end before the same zero-member proof below can pass.
            # An unrelated process is not authority merely because a PID was
            # supplied; Windows explicitly rejects it as this Job's member.
            unrelated = module.open_process(0x1000, False, module.os.getpid())
            self.assertTrue(unrelated)
            try:
                belongs = w.BOOL(True)
                module.base.check(module.is_in_job(unrelated, owner.job, c.byref(belongs)))
                self.assertFalse(belongs.value)
            finally:
                module.base.close(unrelated)
            self.assertTrue(owner.crash()['originalHandleSignalled'])
            deadline = time.monotonic()+5
            while owner.members()['active'] and time.monotonic()<deadline:
                time.sleep(.01)
            self.assertTrue(owner.assert_empty()['emptyBeforeJobClose'])
            self.assertTrue(owner.job, 'outer Job must remain open during proof')
        finally:
            owner.close()

    def test_live_descendant_fails_proof_until_cleanup(self):
        code = ('import json,subprocess,sys,time;'
                'p=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"],creationflags=0x08000000);'
                'print(json.dumps({"child":p.pid}),flush=True);time.sleep(30)')
        owner = module.LauncherJob([sys.executable, '-u', '-c', code])
        handle = None
        try:
            pid = owner.first_line()['child']
            before = owner.members()
            self.assertGreaterEqual(before['active'], 2)
            self.assertIn(owner.process.pid, [m['pid'] for m in before['members']])
            self.assertIn(pid, [m['pid'] for m in before['members']])
            handle = module.open_process(0x1000 | 0x100000, False, pid)
            self.assertTrue(handle)
            owner.crash()
            with self.assertRaisesRegex(RuntimeError, 'left owned Job members'):
                owner.assert_empty()
            self.assertTrue(owner.job)
            self.assertEqual(module.base.wait(handle, 0), 258)
        finally:
            owner.close()
        try:
            self.assertEqual(module.base.wait(handle, 5000), 0,
                             'cleanup must end only our still-live descendant')
        finally:
            if handle:
                module.base.close(handle)


if __name__ == '__main__':
    unittest.main()
