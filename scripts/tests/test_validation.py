"""Offline acceptance tests; no launchctl, GitHub, or real user configuration."""
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'validation.py'


class Acceptance(unittest.TestCase):
    def test_registration_command_is_available(self):
        with tempfile.TemporaryDirectory() as home:
            result = subprocess.run(
                [sys.executable, str(SCRIPT), 'register', '--help'],
                env={**os.environ, 'HOME': home}, capture_output=True, text=True,
            )
        self.assertEqual(result.returncode, 0,
                         '登记命令不可用：register --help 应当成功')

# Import the actual worker without launching external services.
import importlib.util
import io
import plistlib
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('validation', SCRIPT)
v = importlib.util.module_from_spec(spec)
if SCRIPT.exists():
    spec.loader.exec_module(v)


class Child:
    pid = 123

    def __init__(self, output=b'watch ready\n', code=None):
        self.stdout = io.BytesIO(output)
        self.code = code

    def poll(self):
        return self.code

    def wait(self):
        return self.code


class Inhibitor:
    def __init__(self, child):
        self.child = child
        self.waited = False

    def wait(self):
        assert self.child.poll() is not None, '防睡眠必须持续到被测程序结束'
        self.waited = True


class Fake:
    def __init__(self, now=100, child=None):
        self.clock = now
        self.child = child or Child()
        self.starts = []
        self.signals = []
        self.posts = []
        self.inhibitor = None
        self.fail_posts = False
        self.on_sleep = lambda: None

    def now(self):
        return self.clock

    def sleep(self, seconds):
        self.clock += seconds
        self.on_sleep()
        # Give the pipe reader execution time, without any wall-time assertion.
        import time
        time.sleep(0)

    def start(self, task):
        self.starts.append(self.clock)
        return self.child

    def inhibit(self, child):
        assert self.starts, '等待开始时不能阻止空闲睡眠'
        self.inhibitor = Inhibitor(child)
        return self.inhibitor

    def interrupt(self, child):
        self.signals.append((child.pid, self.clock))
        child.code = 0

    def post(self, task, body):
        if self.fail_posts:
            raise RuntimeError('offline')
        self.posts.append((task['issue'], body.read_text()))
        return 'https://github.com/zbtzbtzbt/theFlash/issues/17#test'


class WorkerAcceptance(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = Path(self.tmp.name)
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, {'HOME': str(self.home)}).start()
        self.calls = []
        self.job = v.register(self.home / 'state', self.home / 'agents',
                              [sys.executable, '-c', 'print("ready")'], 400, 600,
                              17, 'zbtzbtzbt/theFlash', '/fake/gh',
                              call=lambda *a, **k: self.calls.append((a, k)))

    def test_register_persists_command_and_launchd_runner_without_shell(self):
        task = v.load(self.job)
        plist = plistlib.loads(Path(task['plist']).read_bytes())
        self.assertEqual(task['status'], 'registered')
        self.assertEqual(task['at'], 400)
        self.assertEqual(task['duration'], 600)
        self.assertEqual(task['cwd'], str(self.job))
        self.assertEqual(task['command'][1:], ['-c', 'print("ready")'])
        self.assertTrue(plist['RunAtLoad'])
        self.assertEqual(plist['ProgramArguments'][-2:], ['_run', str(self.job)])
        self.assertEqual(self.calls[0][0][0][1], 'bootstrap')
        self.assertEqual((self.job.stat().st_mode & 0o777), 0o700)
        self.assertTrue((self.job / 'runner.py').is_file())
        self.assertNotIn('GH_TOKEN', plist['EnvironmentVariables'])

    def test_injected_clock_starts_at_due_time_and_timeout_interrupts_only_child(self):
        fake = Fake()
        v.run_task(self.job, fake)
        self.assertEqual(fake.starts, [400])
        self.assertEqual(len(fake.signals), 1)
        self.assertEqual(fake.signals[0][0], fake.child.pid)
        self.assertGreaterEqual(fake.signals[0][1], 1000)
        self.assertLess(fake.signals[0][1], 1001)
        self.assertTrue(fake.inhibitor.waited)
        task = v.load(self.job)
        self.assertEqual(task['reason'], '达到最长运行时间')
        self.assertEqual(task['exit_code'], 0)
        self.assertEqual(task['delivery'], 'posted')

    def test_cancel_before_due_does_not_start_or_inhibit(self):
        (self.job / 'cancel').touch()
        fake = Fake()
        v.run_task(self.job, fake)
        self.assertEqual(fake.starts, [])
        self.assertIsNone(fake.inhibitor)
        self.assertEqual(v.load(self.job)['status'], 'cancelled')
        self.assertIn('未启动', fake.posts[0][1])

    def test_cancel_running_interrupts_child(self):
        fake = Fake(now=400)
        fake.on_sleep = lambda: (self.job / 'cancel').touch()
        v.run_task(self.job, fake)
        self.assertEqual([p for p, _ in fake.signals], [fake.child.pid])
        self.assertEqual(v.load(self.job)['reason'], '取消')

    def test_summary_contains_times_exit_key_output_and_timestamped_log(self):
        fake = Fake(child=Child(b'watch ready\nall normal\n', code=7))
        v.run_task(self.job, fake)
        text = fake.posts[0][1]
        for expected in ['计划开始：', '实际开始：', '结束：', '退出码：7',
                         'watch ready', 'all normal', str(self.job / 'output.log'),
                         'Agent: validation runner']:
            self.assertIn(expected, text)
        self.assertNotIn('Agent: Codex', text)
        self.assertTrue(all(re.match(r'^\[\d{4}-', line) for line in
                            (self.job / 'output.log').read_text().splitlines()))

    def test_github_failure_retains_result_retry_does_not_rerun_command(self):
        fake = Fake(child=Child(code=0))
        fake.fail_posts = True
        v.run_task(self.job, fake)
        self.assertEqual(v.load(self.job)['delivery'], 'pending')
        self.assertTrue((self.job / 'summary.md').is_file())
        fake.fail_posts = False
        v.run_task(self.job, fake)
        v.run_task(self.job, fake)
        self.assertEqual(len(fake.starts), 1)
        self.assertEqual(len(fake.posts), 1)

    def test_interrupted_runner_is_reported_without_replaying_real_command(self):
        task = v.load(self.job)
        task['status'] = 'running'
        v.save(self.job / 'task.json', task)
        fake = Fake()
        v.run_task(self.job, fake)
        self.assertEqual(fake.starts, [])
        self.assertEqual(v.load(self.job)['status'], 'interrupted')

    def test_protected_state_path_is_refused_before_launchctl(self):
        with self.assertRaises(ValueError, msg='launchd 不能把 runner 放在受保护 Desktop'):
            v.register(self.home / 'Desktop/state', self.home / 'agents',
                       [sys.executable], 100, 10, 17, 'zbtzbtzbt/theFlash',
                       '/fake/gh', call=lambda *a, **k: None)

    def test_launchctl_failure_rolls_back_agent_plist(self):
        def fail(*args, **kwargs):
            raise subprocess.CalledProcessError(1, args[0])
        with self.assertRaises(subprocess.CalledProcessError):
            v.register(self.home / 'failed', self.home / 'failed-agents',
                       [sys.executable], 100, 10, 17, 'zbtzbtzbt/theFlash',
                       '/fake/gh', call=fail)
        self.assertEqual(list((self.home / 'failed-agents').glob('*.plist')), [])

    def test_output_is_bounded_and_sensitive_lines_are_omitted(self):
        raw = ('cookie' + '=private\n<@someone>\n' + 'a' * 10000 + '\n').encode()
        fake = Fake(child=Child(raw, code=0))
        v.run_task(self.job, fake)
        text = (self.job / 'output.log').read_text()
        self.assertNotIn('private', text)
        self.assertLess(max(map(len, text.splitlines())), 2100)
        self.assertIn('&lt;&#64;someone&gt;', fake.posts[0][1])

    def test_launch_intent_is_saved_before_spawning_to_prevent_crash_replay(self):
        fake = Fake(child=Child(code=0))
        start = fake.start
        def check(task):
            self.assertEqual(v.load(self.job)['status'], 'starting')
            return start(task)
        fake.start = check
        v.run_task(self.job, fake)

    def test_sleep_counts_toward_runtime_and_late_wake_is_reported(self):
        fake = Fake(now=500)
        fake.on_sleep = lambda: setattr(fake, 'clock', fake.clock + 10000)
        v.run_task(self.job, fake)
        self.assertEqual(fake.starts, [500])
        self.assertEqual(len(fake.signals), 1)
        self.assertEqual(v.load(self.job)['reason'], '达到最长运行时间')
        self.assertIn(v.stamp(400), fake.posts[0][1])
        self.assertIn(v.stamp(500), fake.posts[0][1])

    def test_inhibitor_start_failure_drains_output_and_stops_child(self):
        fake = Fake(child=Child(b'output\n' * 10000))
        def fail(child):
            raise FileNotFoundError('caffeinate missing')
        fake.inhibit = fail
        v.run_task(self.job, fake)
        self.assertEqual(v.load(self.job)['status'], 'failed')
        self.assertEqual([pid for pid, _ in fake.signals], [fake.child.pid])
        self.assertTrue(fake.child.stdout.closed)

    def test_active_task_cannot_be_uninstalled_or_booted_out(self):
        with patch.object(v.sys, 'argv', ['validation.py', '--state-dir',
                                         str(self.job.parent), 'uninstall', self.job.name]), \
             patch.object(v.sys, 'platform', 'darwin'), \
             patch.object(v.subprocess, 'run') as call:
            with self.assertRaises(ValueError):
                v.main()
            call.assert_not_called()

    def test_log_write_failure_still_interrupts_child_and_closes_pipe(self):
        fake = Fake()
        original_open = Path.open
        class BrokenLog:
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def write(self, line):
                raise OSError('disk full')
        def open_path(path, *args, **kwargs):
            if path.name == 'output.log' and args[:1] == ('a',):
                return BrokenLog()
            return original_open(path, *args, **kwargs)
        with patch.object(Path, 'open', open_path):
            try:
                v.run_task(self.job, fake)
            except OSError:
                pass  # Old code skipped cleanup on a second log-write error.
        self.assertEqual([pid for pid, _ in fake.signals], [fake.child.pid])
        self.assertTrue(fake.child.stdout.closed)
        self.assertEqual(v.load(self.job)['status'], 'failed')
        self.assertEqual(len(fake.posts), 1)

    def test_real_watch_transcript_is_preserved_in_issue_summary(self):
        fixture = SCRIPT.parent / 'testdata/validation/watch-issue17-output.log'
        task = v.load(self.job)
        task.update(status='finished', started=1791915941, ended=1791916541,
                    exit_code=0, reason='达到最长运行时间',
                    key_output=fixture.read_text().splitlines())
        text = v.summary(task, self.job)
        for expected in ['Velosolutions UCI', '158元', '亲子票237元',
                         '每 60 秒查一次', '已停止盯票', '退出码：0']:
            self.assertIn(expected, text)

    def test_start_failure_is_reported(self):
        fake = Fake()
        def fail(task):
            raise FileNotFoundError('gone')
        fake.start = fail
        v.run_task(self.job, fake)
        self.assertEqual(v.load(self.job)['status'], 'failed')
        self.assertEqual(len(fake.posts), 1)

    def test_at_requires_explicit_timezone_and_duration_positive(self):
        with self.assertRaises(ValueError):
            v.parse_at('2026-10-14T10:18:00')
        self.assertEqual(v.parse_at('1970-01-01T00:00:00+00:00'), 0)
        for duration in [0, -1, float('inf'), float('nan')]:
            with self.assertRaises(ValueError):
                v.register(self.home / 'x', self.home / 'a', [sys.executable],
                           100, duration, 17, 'zbtzbtzbt/theFlash', '/fake/gh')


class SystemContract(unittest.TestCase):
    def test_interrupt_is_sigint_to_the_tested_pid_only(self):
        from unittest.mock import Mock
        child = Mock()
        v.System().interrupt(child)
        child.send_signal.assert_called_once_with(v.signal.SIGINT)

    def test_inhibit_watches_pid_without_wrapping_or_signalling_command(self):
        from unittest.mock import Mock
        child = Mock(pid=321)
        with patch.object(v.subprocess, 'Popen') as popen:
            v.System().inhibit(child)
        self.assertEqual(popen.call_args.args[0],
                         ['/usr/bin/caffeinate', '-i', '-w', '321'])
        child.send_signal.assert_not_called()

    def test_github_uses_structured_body_file_not_a_shell(self):
        from unittest.mock import Mock
        with patch.object(v.subprocess, 'run', return_value=Mock(
                returncode=0, stdout='comment-url\n')) as call:
            result = v.System().post(dict(gh='/fake/gh', issue=17,
                                         repo='zbtzbtzbt/theFlash'), Path('/tmp/body'))
        self.assertEqual(result, 'comment-url')
        self.assertEqual(call.call_args.args[0][-2:], ['--body-file', '/tmp/body'])
        self.assertNotIn('shell', call.call_args.kwargs)


if __name__ == '__main__':
    unittest.main()
