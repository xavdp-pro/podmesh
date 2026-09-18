#!/usr/bin/env python3
"""The ledger lock is inherited by the holder's own child, and by no one else.

tools/replicate-universe.py `run` holds a universe's ledger lock and runs `ha-standby.py cycle` as a child, and
`cycle` takes the same lock: without inheritance the child waits for its own parent and every scheduled replication
run is refused as ledger_locked. Runs on the workstation only, against a temporary ledger directory.
"""
import fcntl, importlib.util, json, os, pathlib, subprocess, sys, tempfile, time, unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'tools' / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


U = '00000000-0000-4000-8000-00000000c1c1'
OTHER = '00000000-0000-4000-8000-00000000c2c2'


class LedgerLockInheritance(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix='podmesh-lock-test-')
        os.environ['PODMESH_HA_LEDGER'] = self.dir
        os.environ.pop('PODMESH_HA_LEDGER_LOCK_HELD', None)
        self.replicate = load('replicate_universe', 'replicate-universe.py')
        self.standby = load('ha_standby', 'ha-standby.py')

    def test_a_replication_runs_cycle_is_not_refused_by_its_parents_lock(self):
        with self.replicate.locked(U):
            began = time.monotonic()
            p = subprocess.run([sys.executable, '-B', str(ROOT / 'tools' / 'ha-standby.py'), '--reference', 'test', 'cycle',
                                '--universe', U, '--active', 'nowhere-a', '--standby', 'nowhere-b'],
                               env=dict(os.environ), capture_output=True, text=True, timeout=120)
            elapsed = time.monotonic() - began
        self.assertNotIn('ledger_locked', p.stdout + p.stderr)
        self.assertLess(elapsed, 25, 'the child waited for its parent\'s lock')

    def test_a_process_that_does_not_hold_the_lock_still_waits_and_is_refused(self):
        path = pathlib.Path(self.dir) / f'{U}.lock'
        with open(path, 'a+') as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            os.environ['PODMESH_HA_LEDGER_LOCK_HELD'] = f'{OTHER}:{os.getppid()}'   # another universe: inherits nothing
            try:
                with self.assertRaises(self.standby.Refusal) as refused:
                    with self.standby.locked(U, wait_seconds=1):
                        pass
            finally:
                os.environ.pop('PODMESH_HA_LEDGER_LOCK_HELD', None)
        self.assertIn('ledger_locked', str(refused.exception))

    def test_the_holder_names_the_universe_only_while_it_holds_the_lock(self):
        for module in (self.replicate, self.standby):
            with module.locked(U):
                self.assertEqual(os.environ.get('PODMESH_HA_LEDGER_LOCK_HELD'), f'{U}:{os.getpid()}')
            self.assertIsNone(os.environ.get('PODMESH_HA_LEDGER_LOCK_HELD'))

    def test_a_grandchild_or_a_forged_pid_inherits_nothing(self):
        path = pathlib.Path(self.dir) / f'{U}.lock'
        with open(path, 'a+') as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            for held in (f'{U}:{os.getpid()}', f'{U}:1', U):   # the process's own pid, init, no pid at all
                os.environ['PODMESH_HA_LEDGER_LOCK_HELD'] = held
                try:
                    with self.assertRaises(self.standby.Refusal):
                        with self.standby.locked(U, wait_seconds=0.6):
                            pass
                finally:
                    os.environ.pop('PODMESH_HA_LEDGER_LOCK_HELD', None)

    def test_a_second_process_is_refused_while_the_first_holds_it(self):
        with self.replicate.locked(U):
            env = dict(os.environ)
            env.pop('PODMESH_HA_LEDGER_LOCK_HELD')   # an unrelated process, not the holder's child
            code = ('import importlib.util, sys; sys.path.insert(0, %r); '
                    's = importlib.util.spec_from_file_location("h", %r); m = importlib.util.module_from_spec(s); '
                    's.loader.exec_module(m)\n'
                    'try:\n    with m.locked(%r, wait_seconds=1): print("taken")\n'
                    'except m.Refusal as e: print("refused", e)') % (str(ROOT / 'tests'), str(ROOT / 'tools' / 'ha-standby.py'), U)
            p = subprocess.run([sys.executable, '-B', '-c', code], env=env, capture_output=True, text=True, timeout=60)
        self.assertTrue(p.stdout.startswith('refused'), p.stdout + p.stderr)


if __name__ == '__main__':
    unittest.main()
