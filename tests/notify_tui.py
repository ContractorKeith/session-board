"""Exercise notifications through the real scan channel and TUI event loop."""
from datetime import datetime, timezone
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
from terminal import screen

binary = str(Path(sys.argv[1]).resolve())
for mode in ('slow', 'disabled', 'fail', 'timeout'):
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        for directory in ('claude/projects/project', 'codex/sessions', 'config', 'bin'):
            (root / directory).mkdir(parents=True)
        (root / 'config/config.toml').write_text('notify = ' + ('false' if mode == 'disabled' else 'true') + '\n')
        transcript = root / 'claude/projects/project/session.jsonl'
        log = root / 'calls'
        def fixture(status):
            rows = [json.loads(line) for line in Path(f'fixtures/claude_code/project/{status}.jsonl').read_text().splitlines()]
            for row in rows:
                row.update(sessionId='stable', cwd=str(root), timestamp=datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'))
            transcript.write_text(''.join(json.dumps(row) + '\n' for row in rows))
        fixture('working')
        # A real executable boundary, never a real desktop server.
        for name in ('notify-send', 'osascript'):
            fake = root / 'bin' / name
            fake.write_text('#!/bin/sh\nprintf "call\\n" >> "$NOTIFY_LOG"\n' + ('exec /bin/sleep 3\n' if mode == 'slow' else 'exec /bin/sleep 30\n' if mode == 'timeout' else 'exit 1\n' if mode == 'fail' else 'exit 0\n'))
            fake.chmod(0o755)
        env = dict(os.environ, TERM='xterm-256color', CLAUDE_CONFIG_DIR=str(root / 'claude'),
                   CODEX_HOME=str(root / 'codex'), SBOARD_STATE_DIR=str(root / 'state'),
                   SBOARD_CONFIG_DIR=str(root / 'config'), NOTIFY_LOG=str(log), PATH=str(root / 'bin') + ':' + os.environ['PATH'])
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        proc = subprocess.Popen([binary], env=env, stdin=slave, stdout=slave, stderr=subprocess.PIPE)
        os.close(slave)
        output = b''
        def pump():
            global output
            if select.select([master], [], [], .005)[0]:
                output += os.read(master, 65536)
        def wait(predicate, description, timeout=6):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                pump()
                if predicate():
                    return
                if proc.poll() is not None:
                    break
            raise AssertionError(f'{mode}: {description}\n{screen(output)}')
        try:
            wait(lambda: '1 projects' in screen(output), 'initial scan missing')
            assert not log.exists(), 'initial scan notified'
            fixture('waiting')
            if mode == 'disabled':
                wait(lambda: 'waiting' in screen(output), 'transition scan missing')
                until = time.monotonic() + .15
                while time.monotonic() < until:
                    pump()
                assert not log.exists(), 'notify=false delivered a notification'
            else:
                wait(log.exists, 'transition never reached the notifier')
                assert log.read_text().splitlines() == ['call'], 'transition delivered more than once'
                if mode == 'slow':
                    offset = len(output)
                    sent = time.monotonic()
                    os.write(master, b'?')
                    # Time the visible Help title, excluding Python's full screen reconstruction.
                    wait(lambda: b'Help' in output[offset:], 'help blocked behind desktop command', timeout=.1)
                    elapsed = time.monotonic() - sent
                    assert elapsed < .1, f'help exceeded 100 ms: {elapsed:.3f}s'
                    wait(lambda: 'Attention focus: Enter' in screen(output), 'help frame incomplete')
                    print(f'help visible in {elapsed * 1000:.1f} ms')
                else:
                    wait(lambda: 'desktop notification:' in screen(output), 'delivery failure missing from status bar')
                    command = 'osascript' if sys.platform == 'darwin' else 'notify-send'
                    assert command in screen(output).splitlines()[-1]
                    assert '1 warnings:' in screen(output).splitlines()[-1]
            os.write(master, b'q')
            proc.wait(timeout=2)
            assert proc.returncode == 0
            errors = proc.stderr.read()
            assert not errors, f'notifier wrote over terminal: {errors!r}'
            print(f'{mode}: real scan/event path passed')
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            proc.stderr.close()
            os.close(master)
