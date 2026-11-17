"""Actual native-AMD64 Windows fault-helper tests with synthetic owned processes."""
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import uuid


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


hold = load('owned_hold', 'hold-owned-primitive-worker.py')
job = load('owned_fixture_job', 'owned-launcher-job.py')


def line(child, timeout=15):
    result = queue.Queue(maxsize=1)

    def read():
        try:
            raw = child.stdout.readline(65537)
            if not raw or len(raw) > 65536:
                raise RuntimeError('missing bounded helper message')
            result.put(json.loads(raw))
        except BaseException as error:
            result.put(error)

    threading.Thread(target=read, daemon=True).start()
    value = result.get(timeout=timeout)
    if isinstance(value, BaseException):
        raise value
    return value


class HoldTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        node = os.environ.get('DIY_HOLD_TEST_NODE') or shutil.which('node')
        if not node:
            raise RuntimeError('explicit or PATH Node required for synthetic fixture')
        cls.parent = Path(tempfile.gettempdir()).resolve(strict=True)
        cls.root = Path(tempfile.mkdtemp(prefix='diyime-primitive-retention-unit-', dir=cls.parent)).resolve(strict=True)
        cls.workers = cls.root/'workers'
        cls.workers.mkdir()
        cls.node_source = Path(node).resolve(strict=True)
        cls.digest = hold.digest_file(cls.node_source)
        cls.cleanup_waits = []

    @classmethod
    def tearDownClass(cls):
        resolved = cls.root.resolve(strict=True)
        if resolved.parent != cls.parent or not resolved.name.startswith('diyime-primitive-retention-unit-'):
            raise RuntimeError('unsafe owned fixture cleanup')
        shutil.rmtree(resolved)
        if cls.cleanup_waits:
            print(json.dumps({'fixtureCleanupWaits': cls.cleanup_waits}), flush=True)

    def setUp(self):
        self.fixed_root = None
        if self._testMethodName == 'test_original_handle_termination_ends_helper_and_releases_dead_references':
            self.fixed_root = Path(tempfile.mkdtemp(prefix='diyime-retained-host-unit-', dir=self.parent)).resolve(strict=True)
            self.workers = self.fixed_root/'workers'
            self.workers.mkdir()
        folder = self.workers/('DIYIME2.Worker.'+uuid.uuid4().hex)
        folder.mkdir()
        worker = folder/'worker.exe'
        # Independent image per case: executable hard-link aliases can remain
        # locked even after the original process and Job have ended.
        shutil.copyfile(self.node_source, worker)
        if os.path.samefile(self.node_source, worker):
            raise RuntimeError('fixture image must have its own file identity')
        self.worker = worker
        self.owner = job.LauncherJob([str(worker), '-e',
            'process.stdout.write(JSON.stringify({ready:true})+"\\n");setInterval(()=>{},1000)'])
        self.assertEqual(self.owner.first_line(), {'ready': True})
        self.helpers = []

    def tearDown(self):
        for child in self.helpers:
            if child.stdin and not child.stdin.closed:
                child.stdin.close()
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
            for stream in (child.stdout, child.stderr):
                if stream:
                    stream.close()
        try:
            if hold.alive(self.owner.process.handle):
                self.owner.crash()
            deadline = time.monotonic()+5
            while self.owner.members()['active'] and time.monotonic()<deadline:
                time.sleep(.01)
            self.assertTrue(self.owner.assert_empty()['emptyBeforeJobClose'])
        finally:
            self.owner.close()
        resolved = self.worker.resolve(strict=True)
        if self.workers not in resolved.parents or resolved.name != 'worker.exe':
            raise RuntimeError('unsafe owned executable cleanup')
        # Original handle exit AND strict Job zero are already proven above.
        # Windows may briefly retain an image/scan file reference afterwards;
        # retry only this verified fixture file, and still fail at the bound.
        started, attempts = time.monotonic(), 0
        while True:
            attempts += 1
            try:
                resolved.unlink()
                break
            except PermissionError as error:
                if getattr(error, 'winerror', None) not in (5, 32) or time.monotonic()-started >= 3:
                    raise
                time.sleep(.05)
        if attempts > 1:
            self.cleanup_waits.append({'case': self._testMethodName, 'attempts': attempts,
                                       'elapsedMs': (time.monotonic()-started)*1000})
        resolved.parent.rmdir()
        if self.fixed_root:
            self.workers.rmdir()
            self.fixed_root.rmdir()

    def helper(self, milliseconds=5000):
        child = subprocess.Popen([sys.executable, str(Path(__file__).with_name('hold-owned-primitive-worker.py')),
                                  str(self.owner.process.pid), str(self.workers), self.digest,
                                  '--hold-ms', str(milliseconds)],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 creationflags=0x08000000)
        self.helpers.append(child)
        started = line(child)
        self.assertEqual(started['event'], 'held')
        self.assertEqual(started['pid'], self.owner.process.pid)
        self.assertGreater(started['threads'], 0)
        self.assertLessEqual(started['threads'], 128)
        return child, started

    def test_confirmed_original_threads_restore_exact_increments(self):
        process, created = hold.checked_worker(self.owner.process.pid, self.workers, self.digest)
        held = None
        try:
            self.assertTrue(created.isdecimal())
            held = hold.HeldThreads(process)
            count = len(held.threads)
            self.assertGreater(count, 0)
            self.assertTrue(hold.alive(process))
            self.assertEqual(held.release(), count)
            self.assertFalse(held.forced_cleanup)
        finally:
            if held:
                held.release()
            hold.base.close(process)

    def test_eof_resumes_and_does_not_claim_original_exit(self):
        child, started = self.helper()
        child.stdin.close()
        released = line(child)
        code = child.wait(timeout=5)
        evidence = child.stderr.read().decode(errors='replace')
        self.assertEqual(code, 0, evidence)
        self.assertEqual(evidence.splitlines(), [
            'DIYIME_HOLD_SETUP ' + stage for stage in (
                'python-entered', 'native-api-ready', 'original-handle-opened',
                'image-verified', 'original-identity-verified', 'suspension-started',
                'suspension-confirmed')])
        self.assertEqual(released['event'], 'released')
        self.assertEqual(released['reason'], 'eof-or-release')
        self.assertFalse(released['originalHandleSignalled'])
        self.assertFalse(released['forcedCleanup'])
        self.assertEqual(released['resumed'], started['threads'])
        self.assertTrue(hold.alive(self.owner.process.handle))

    def test_timeout_resumes_and_cannot_count_as_host_cancellation(self):
        child, started = self.helper(100)
        released = line(child)
        self.assertEqual(child.wait(timeout=5), 0, child.stderr.read().decode(errors='replace'))
        self.assertEqual(released['reason'], 'timeout')
        self.assertFalse(released['originalHandleSignalled'])
        self.assertFalse(released['forcedCleanup'])
        self.assertEqual(released['resumed'], started['threads'])
        self.assertTrue(hold.alive(self.owner.process.handle))

    def test_original_handle_termination_ends_helper_and_releases_dead_references(self):
        child, _ = self.helper()
        self.owner.crash()
        released = line(child)
        self.assertEqual(child.wait(timeout=5), 0, child.stderr.read().decode(errors='replace'))
        self.assertEqual(released['reason'], 'original-exit')
        self.assertTrue(released['originalHandleSignalled'])
        self.assertFalse(released['forcedCleanup'])
        # Keep the original process handle open so its PID cannot be reused.
        # A stale status PID must be refused before image/thread inspection.
        self.assertFalse(hold.alive(self.owner.process.handle))
        with self.assertRaisesRegex(ValueError, 'original Worker already ended'):
            hold.checked_worker(self.owner.process.pid, self.workers, self.digest)

    def test_wrong_digest_and_unrelated_image_are_refused_before_hold(self):
        with self.assertRaisesRegex(ValueError, 'private benchmark workspace required'):
            hold.checked_worker(self.owner.process.pid, self.workers.parent, self.digest)
        with self.assertRaisesRegex(ValueError, 'digest differs'):
            hold.checked_worker(self.owner.process.pid, self.workers, '0'*64)
        foreign = job.LauncherJob([sys.executable, '-u', '-c',
                                  'import json,time;print(json.dumps({"ready":True}),flush=True);time.sleep(30)'])
        try:
            foreign.first_line()
            with self.assertRaisesRegex(ValueError, 'not an exact workspace Worker'):
                hold.checked_worker(foreign.process.pid, self.workers, self.digest)
            self.assertTrue(hold.alive(foreign.process.handle))
            foreign.crash()
        finally:
            foreign.close()
        self.assertTrue(hold.alive(self.owner.process.handle))


if __name__ == '__main__':
    unittest.main()
