#!/usr/bin/env python3
"""One-shot macOS launchd validations. Standard library only; effects injectable."""
import argparse
from collections import deque
from datetime import datetime, timezone
import fcntl
import html
import json
import math
import os
from pathlib import Path
import plistlib
import queue
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import uuid

PREFIX = 'cn.theflash.validation.'
TERMINAL = {'finished', 'cancelled', 'interrupted', 'failed'}


def stamp(seconds):
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat(timespec='seconds')


def save(path, data):
    tmp = path.with_suffix('.tmp')
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
    tmp.replace(path)


def load(job):
    return json.loads((job / 'task.json').read_text())


def safe_line(line):
    # Drop whole sensitive lines before either logging or posting. Commands
    # must themselves avoid secrets: arbitrary output cannot be fully classified.
    if re.search(r'(?i)cookie|authorization|token|password|密码|身份证|手机号', line):
        return '[敏感字段所在行已省略]'
    return ''.join(c for c in line if c == '\t' or c.isprintable())[:2000]


def summary(task, job):
    output = '\n'.join(task.get('key_output', [])) or '（无输出）'
    exit_text = (task['exit_code'] if task.get('exit_code') is not None
                 else '未知' if task.get('started') is not None else '未启动')
    return (f"### 验证任务 {task['id']}\n\n"
            f"- 状态：{task['status']}；停止原因：{task.get('reason', '无')}\n"
            f"- 计划开始：{stamp(task['at'])}\n"
            f"- 实际开始：{stamp(task['started']) if task.get('started') is not None else '未启动'}\n"
            f"- 结束：{stamp(task['ended'])}\n"
            f"- 退出码：{exit_text}\n"
            f"- 日志位置（本机）：`{job / 'output.log'}`\n\n"
            f"关键输出（最多前 8 / 后 12 行；完整输出在本机日志）：\n"
            f"<pre>{html.escape(output).replace('@', '&#64;')}</pre>\n\n"
            f"<!-- theflash-validation:{task['id']} -->\nAgent: validation runner\n")


class System:
    def now(self):
        return time.time()

    def sleep(self, seconds):
        time.sleep(seconds)

    def start(self, task):
        return subprocess.Popen(task['command'], cwd=task['cwd'],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                stdin=subprocess.DEVNULL,
                                env={**os.environ, 'PYTHONUNBUFFERED': '1'})

    def inhibit(self, child):
        return subprocess.Popen(['/usr/bin/caffeinate', '-i', '-w', str(child.pid)],
                                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL)

    def interrupt(self, child):
        try:
            child.send_signal(signal.SIGINT)
        except ProcessLookupError:
            pass

    def post(self, task, body):
        # Resolve on delivery, including legacy tasks pinned to a removed Cellar
        # version. Keep the PATH symlink rather than its versioned target.
        gh = shutil.which('gh') or task['gh']
        result = subprocess.run([gh, 'issue', 'comment', str(task['issue']),
                                 '--repo', task['repo'], '--body-file', str(body)],
                                capture_output=True, text=True, timeout=60)
        if result.returncode:
            raise RuntimeError('GitHub 评论失败；详情不写入日志以避免凭据泄漏')
        return result.stdout.strip()


def deliver(job, task, effects, force=False):
    if task.get('delivery') in {'posted', 'abandoned'}:
        return task['delivery'] == 'posted'
    if not force and effects.now() < task.get('next_delivery', 0):
        return False
    body = job / 'summary.md'
    body.write_text(summary(task, job))
    try:
        task['comment_url'] = effects.post(task, body)
    except (OSError, RuntimeError, subprocess.TimeoutExpired):
        failures = min(task.get('delivery_failures', 0) + 1, 5)
        task.update(delivery='pending', delivery_failures=failures,
                    next_delivery=effects.now() + min(60 * 2 ** (failures - 1), 900),
                    delivery_error='GitHub 评论失败；请检查网络、gh 安装及登录状态')
        save(job / 'task.json', task)
        return False
    task.update(delivery='posted', delivery_failures=0)
    task.pop('next_delivery', None)
    task.pop('delivery_error', None)
    save(job / 'task.json', task)
    return True


def logged_output(job):
    first, last = [], deque(maxlen=12)
    path = job / 'output.log'
    if path.exists():
        with path.open(errors='replace') as log:
            while line := log.readline(8192):
                line = safe_line(line.rstrip())
                if len(first) < 8:
                    first.append(line)
                else:
                    last.append(line)
    return first + list(last)


def run_task(job, effects=None, cancelled=None):
    effects = effects or System()
    cancelled = cancelled or (lambda: False)
    task = load(job)
    if task['status'] in TERMINAL:
        deliver(job, task, effects)
        return
    if task['status'] != 'registered':
        # A previous runner died. Replaying a real-world command is unsafe.
        task.update(status='interrupted', reason='runner 中断；没有重新运行命令',
                    ended=effects.now(), key_output=logged_output(job))
        save(job / 'task.json', task)
        deliver(job, task, effects)
        return

    def stop_requested():
        return cancelled() or (job / 'cancel').exists()

    while effects.now() < task['at'] and not stop_requested():
        effects.sleep(min(1, task['at'] - effects.now()))
    first, last = [], deque(maxlen=12)
    messages = queue.Queue(maxsize=256)
    child = inhibitor = reader = None
    started = None
    reason = '程序自行退出'
    status = 'finished'
    exit_code = None
    with (job / 'output.log').open('a', buffering=1) as log:
        def record(line):
            text = f'[{stamp(effects.now())}] {safe_line(line.rstrip())}'
            log.write(text + '\n')
            if len(first) < 8:
                first.append(text)
            else:
                last.append(text)

        try:
            if stop_requested():
                status, reason = 'cancelled', '开始前取消'
            else:
                # Record intent before spawning: a crash in the narrow window
                # between spawn and saving the PID must not replay a real command.
                task['status'] = 'starting'
                save(job / 'task.json', task)
                child = effects.start(task)
                started = effects.now()
                task.update(status='running', started=started, pid=child.pid)
                save(job / 'task.json', task)
                record('被测程序已启动')
                def read_output():
                    # Cap even an unterminated line; never buffer unlimited output.
                    while True:
                        raw = child.stdout.readline(8192)
                        if not raw:
                            break
                        messages.put(raw.decode('utf-8', errors='replace'))
                    child.stdout.close()

                reader = threading.Thread(target=read_output, daemon=True)
                reader.start()
                inhibitor = effects.inhibit(child)
                interrupted = False
                while child.poll() is None or reader.is_alive() or not messages.empty():
                    # Bound draining so noisy output cannot starve the deadline.
                    for _ in range(256):
                        try:
                            record(messages.get_nowait())
                        except queue.Empty:
                            break
                    if child.poll() is None and not interrupted:
                        if stop_requested() or effects.now() >= started + task['duration']:
                            reason = '取消' if stop_requested() else '达到最长运行时间'
                            status = 'cancelled' if stop_requested() else 'finished'
                            record(reason + '；仅向被测程序发送 SIGINT')
                            effects.interrupt(child)
                            interrupted = True
                            task.update(status='stopping', reason=reason)
                            save(job / 'task.json', task)
                    effects.sleep(0.1)
                exit_code = child.wait()
                reader.join()
        except (OSError, RuntimeError) as error:
            status, reason = 'failed', f'启动或监督失败：{type(error).__name__}'
            if child and child.poll() is None:
                effects.interrupt(child)
            # A failed log write must still stop the child. Do not let a second
            # write failure bypass cleanup and leave caffeinate waiting forever.
            def record_failure(line):
                try:
                    record(line)
                except OSError:
                    pass

            record_failure(reason)
            if reader:
                while child.poll() is None or reader.is_alive() or not messages.empty():
                    for _ in range(256):
                        try:
                            record_failure(messages.get_nowait())
                        except queue.Empty:
                            break
                    effects.sleep(0.1)
                reader.join()
            elif child:
                child.stdout.close()
            if child:
                child.wait()
        finally:
            if inhibitor:
                # Never signal caffeinate: -w releases its assertion on child exit.
                inhibitor.wait()
        try:
            record(f'任务结束；退出码：{exit_code}')
        except OSError:
            status, reason = 'failed', '日志写入失败；被测程序已停止'
    task.update(status=status, reason=reason, exit_code=exit_code,
                ended=effects.now(), key_output=first + list(last))
    if started is not None:
        task['started'] = started
    save(job / 'task.json', task)
    deliver(job, task, effects)


def register(root, agents, command, at, duration, issue, repo, gh, call=None,
             cwd=None):
    call = call or subprocess.run
    if not math.isfinite(at) or not math.isfinite(duration) or duration <= 0 or issue <= 0:
        raise ValueError('开始时间、运行时长和 issue 必须有效；时长大于零')
    if at < time.time() - 5:
        raise ValueError('开始时间已经过去超过 5 秒；请核对日期，立即开始请用 --at now')
    if cwd is not None:
        cwd = cwd.expanduser().resolve()
        if not cwd.is_dir():
            raise ValueError('--cwd 必须是已存在的目录')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
        raise ValueError('repo 必须是 owner/repository')
    if not command:
        raise ValueError('-- 后必须提供被测命令')
    # launchd does not inherit Terminal's Desktop/Documents privacy grant.
    # A runner under these paths can block before opening its state/log file.
    for folder in ('Desktop', 'Documents', 'Downloads'):
        if root.resolve().is_relative_to(Path.home().resolve() / folder):
            raise ValueError('任务状态目录不能在 Desktop/Documents/Downloads；请用默认目录')
    executable = shutil.which(command[0])
    if not executable:
        raise ValueError('找不到被测程序')
    task_id = uuid.uuid4().hex[:12]
    label = PREFIX + task_id
    job = root / task_id
    job.mkdir(parents=True, mode=0o700)
    agents.mkdir(parents=True, exist_ok=True)
    runner = job / 'runner.py'
    shutil.copyfile(Path(__file__).resolve(), runner)
    task = dict(id=task_id, label=label, command=[str(Path(executable).resolve()), *command[1:]],
                at=at, duration=duration, issue=issue, repo=repo, gh=os.path.abspath(gh),
                cwd=str(cwd if cwd else job), status='registered', delivery='pending',
                plist=str(agents / (label + '.plist')))
    save(job / 'task.json', task)
    plist = dict(Label=label, ProgramArguments=[sys.executable, str(runner),
                 '_run', str(job)], RunAtLoad=True, StartInterval=60,
                 WorkingDirectory=str(job), ProcessType='Standard',
                 StandardOutPath=str(job / 'runner.log'),
                 StandardErrorPath=str(job / 'runner.log'),
                 EnvironmentVariables={'HOME': str(Path.home()),
                                       'PATH': os.environ.get('PATH', '/usr/bin:/bin')})
    path = Path(task['plist'])
    path.write_bytes(plistlib.dumps(plist))
    path.chmod(0o600)
    try:
        call(['/bin/launchctl', 'bootstrap', f'gui/{os.getuid()}', str(path)], check=True)
    except (OSError, subprocess.CalledProcessError):
        path.unlink(missing_ok=True)
        task.update(status='failed', reason='launchctl bootstrap 失败', ended=time.time())
        save(job / 'task.json', task)
        raise
    return job


def parse_at(value):
    if value == 'now':
        return time.time()
    date = datetime.fromisoformat(value)
    if date.tzinfo is None:
        raise ValueError('--at 必须含时区，例如 2026-10-14T10:18:00+08:00')
    return date.timestamp()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--state-dir', type=Path,
                        default=Path.home() / 'Library/Application Support/theFlash/validation')
    sub = parser.add_subparsers(dest='action', required=True)
    reg = sub.add_parser('register')
    reg.add_argument('--at', default='now', help='now 或带时区的 ISO 时间')
    reg.add_argument('--duration', type=float, required=True, help='运行秒数')
    reg.add_argument('--issue', type=int, required=True)
    reg.add_argument('--repo', default='zbtzbtzbt/theFlash')
    reg.add_argument('--cwd', type=Path, help='被测命令的工作目录，默认独立任务目录')
    reg.add_argument('command', nargs=argparse.REMAINDER, help='-- 命令 参数…')
    sub.add_parser('list')
    for name in ['cancel', 'uninstall', 'report']:
        action = sub.add_parser(name)
        action.add_argument('id')
        if name == 'uninstall':
            action.add_argument('--discard-report', action='store_true',
                                help='仅对已结束任务：放弃自动补发，卸载并保留本机证据')
    sub.add_parser('_run').add_argument('job', type=Path)
    args = parser.parse_args()
    if args.action == '_run':
        # Only one runner may own a job. Keep the lock throughout supervision.
        with (args.job / 'runner.lock').open('a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            stopping = threading.Event()
            for sig in (signal.SIGTERM, signal.SIGINT):
                signal.signal(sig, lambda *_: stopping.set())
            run_task(args.job, cancelled=stopping.is_set)
        return
    if sys.platform != 'darwin':
        raise ValueError('launchd 登记和管理只支持 macOS；Linux 可跑离线测试')
    root = args.state_dir.expanduser().resolve()
    if args.action == 'register':
        gh = shutil.which('gh')
        if not gh:
            raise ValueError('需要已登录的 gh（不保存凭据到任务）')
        command = args.command[1:] if args.command[:1] == ['--'] else args.command
        job = register(root, Path.home() / 'Library/LaunchAgents', command,
                       parse_at(args.at), args.duration, args.issue, args.repo, gh,
                       cwd=args.cwd)
        print(f'已登记 {job.name}；日志和状态：{job}')
    elif args.action == 'list':
        for path in sorted(root.glob('*/task.json')):
            task = load(path.parent)
            print(f"{task['id']} {task['status']} {stamp(task['at'])} "
                  f"#{task['issue']} delivery={task['delivery']}")
    else:
        if not re.fullmatch(r'[a-f0-9]{12}', args.id):
            raise ValueError('无效任务 ID')
        job = root / args.id
        task = load(job)
        if args.action == 'cancel':
            (job / 'cancel').touch()
            print('已请求取消；runner 仅向被测程序发送 SIGINT，请用 list 确认结束')
        elif args.action == 'report':
            with (job / 'runner.lock').open('a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                task = load(job)
                if task['status'] not in TERMINAL:
                    raise ValueError('任务还没有结束')
                if task['delivery'] == 'abandoned':
                    raise ValueError('已放弃自动发送；摘要和日志保留在任务目录')
                if not deliver(job, task, System(), force=True):
                    raise RuntimeError('结果仍待发送；请检查网络、gh 安装及登录状态；摘要保留在任务目录')
                print(task['delivery'])
        else:
            # Refuse bootout while runner is still posting or supervising.
            with (job / 'runner.lock').open('a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                task = load(job)
                if task['status'] not in TERMINAL:
                    raise ValueError('请先 cancel，等任务结束后再卸载；不能卸载活动任务')
                if task['delivery'] not in {'posted', 'abandoned'} and not args.discard_report:
                    raise ValueError('结果尚未送达；等待自动重试，或 report；放弃发送请用 --discard-report')
                result = subprocess.run(['/bin/launchctl', 'bootout',
                                         f"gui/{os.getuid()}/{task['label']}"],
                                        capture_output=True, text=True)
                if result.returncode and 'No such process' not in result.stderr:
                    raise RuntimeError('launchctl bootout 失败；保留 plist 以供重试')
                Path(task['plist']).unlink(missing_ok=True)
                if task['delivery'] != 'posted':
                    (job / 'summary.md').write_text(summary(task, job))
                    task['delivery'] = 'abandoned'
                    save(job / 'task.json', task)
            print('已卸载；保留日志和摘要')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f'验证任务操作失败：{type(exc).__name__}: {exc}', file=sys.stderr)
        sys.exit(1)
