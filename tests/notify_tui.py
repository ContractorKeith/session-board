"""Exercise notifications through the real scan channel and TUI event loop."""
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import pty
import struct
import subprocess
import sys
import tempfile
import termios
import time
from terminal import screen
from pty_probe import deadline, read_ready, wait_exit

binary = str(Path(sys.argv[1]).resolve())
for mode in ([sys.argv[2]] if len(sys.argv) > 2 else ('slow', 'disabled', 'fail', 'timeout', 'scan-start')):
    stage = 'setup'
    with deadline(lambda: f'tui_notifications_use_real_scan_events_and_config_without_blocking_help [{mode}]: waiting for {stage}'), tempfile.TemporaryDirectory() as scratch, tempfile.TemporaryFile() as errors_file:
        root = Path(scratch)
        for directory in ('claude/projects/project', 'codex/sessions', 'config', 'bin'):
            (root / directory).mkdir(parents=True)
        (root / 'config/config.toml').write_text('notify = ' + ('false' if mode == 'disabled' else 'true') + '\n')
        transcript = root / 'claude/projects/project/session.jsonl'
        log = root / 'calls'
        def fixture(status, target=None, identity='stable'):
            rows = [json.loads(line) for line in Path(f'fixtures/claude_code/project/{status}.jsonl').read_text().splitlines()]
            for row in rows:
                row.update(sessionId=identity, cwd=str(root), timestamp=datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'))
            (target or transcript).write_text(''.join(json.dumps(row) + '\n' for row in rows))
        gate = root / 'gate'
        os.mkfifo(gate)
        gate_reader = os.open(gate, os.O_RDONLY | os.O_NONBLOCK)
        gate_fd = os.open(gate, os.O_WRONLY | os.O_NONBLOCK)
        fixture('working')
        entered = root / 'git-entered'
        if mode == 'scan-start':
            git = root / 'bin/git'
            git.write_text('#!/bin/sh\nprintf ready > "$GIT_ENTERED"\nread release < "$NOTIFY_GATE"\nexit 1\n')
            git.chmod(0o755)
        # A real executable boundary, never a real desktop server.
        for name in ('notify-send', 'osascript'):
            fake = root / 'bin' / name
            fake.write_text('#!/bin/sh\nprintf "call\\n" >> "$NOTIFY_LOG"\n' + ('read ignored < "$NOTIFY_GATE"\n' if mode == 'slow' else 'exec /bin/sleep 30\n' if mode == 'timeout' else 'exit 1\n' if mode == 'fail' else 'exit 0\n'))
            fake.chmod(0o755)
        env = dict(os.environ, TERM='xterm-256color', CLAUDE_CONFIG_DIR=str(root / 'claude'),
                   CODEX_HOME=str(root / 'codex'), SBOARD_STATE_DIR=str(root / 'state'),
                   SBOARD_CONFIG_DIR=str(root / 'config'), NOTIFY_LOG=str(log), NOTIFY_GATE=str(gate), GIT_ENTERED=str(entered), PATH=str(root / 'bin') + ':' + os.environ['PATH'])
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        def controlling_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        stage = 'launch board'
        proc = subprocess.Popen([binary], env=env, stdin=slave, stdout=slave, stderr=errors_file,
                                preexec_fn=controlling_terminal)
        os.close(slave)
        output = b''
        def pump():
            global output
            output += read_ready(master)
        def wait(predicate, description, timeout=15):
            global stage
            stage = description
            print(f'{mode}: waiting for {stage}', flush=True)
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                pump()
                if predicate():
                    return
                if proc.poll() is not None:
                    break
            raise AssertionError(f'{mode}: {description}\n{screen(output)}')
        try:
            if mode == 'scan-start':
                wait(entered.exists, 'initial source enumeration never reached git')
                created = int(time.time())
                fixture('waiting', root / 'claude/projects/project/new.jsonl', 'new-session')
                wait(lambda: int(time.time()) > created, 'clock did not cross the creation second')
                os.write(gate_fd, b'release\n')
            wait(lambda: '1 projects' in screen(output), 'initial scan missing')
            if mode != 'scan-start':
                assert not log.exists(), 'initial scan notified'
                fixture('waiting')
            if mode == 'disabled':
                wait(lambda: 'waiting' in screen(output), 'transition scan missing')
                until = time.monotonic() + .5
                while time.monotonic() < until:
                    pump()
                assert not log.exists(), 'notify=false delivered a notification'
            else:
                wait(lambda: log.exists() and log.read_text().splitlines() == ['call'], 'transition never reached the notifier')
                assert log.read_text().splitlines() == ['call'], 'transition delivered more than once'
                if mode == 'slow':
                    offset = len(output)
                    sent = time.monotonic()
                    os.write(master, b'?')
                    # Time the visible Help title, excluding Python's full screen reconstruction.
                    wait(lambda: b'Help' in output[offset:], 'help blocked behind desktop command', timeout=2)
                    elapsed = time.monotonic() - sent
                    # The notifier is held until Help appears; no scheduler-sensitive stopwatch.
                    os.write(gate_fd, b'release\n')
                    wait(lambda: 'Attention focus: Enter' in screen(output), 'help frame incomplete')
                    print(f'help visible in {elapsed * 1000:.1f} ms')
                elif mode != 'scan-start':
                    wait(lambda: 'desktop notification:' in screen(output), 'delivery failure missing from status bar')
                    command = 'osascript' if sys.platform == 'darwin' else 'notify-send'
                    assert command in screen(output).splitlines()[-1]
                    assert '1 warnings:' in screen(output).splitlines()[-1]
            stage = 'quit acknowledgement'
            os.write(master, b'q')
            wait_exit(proc, pump, f'{mode}: {stage}', 5)
            assert proc.returncode == 0
            stage = 'stderr check after exit'
            errors_file.seek(0)
            errors = errors_file.read(65536)
            assert not errors, f'notifier wrote over terminal: {errors!r}'
            print(f'{mode}: real scan/event path passed')
        finally:
            if proc.poll() is None:
                # Allow the board to cancel/reap its notifier group on assertion failures too.
                try:
                    os.write(master, b'q')
                    wait_exit(proc, pump, f'{mode}: cleanup', 8)
                except (OSError, subprocess.TimeoutExpired):
                    proc.kill()
                    wait_exit(proc, pump, f'{mode}: {stage}', 5)
            os.close(master)
            os.close(gate_fd)
            os.close(gate_reader)
