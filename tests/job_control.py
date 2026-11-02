"""A stopped foreground agent must let the caller's shell stop/resume the board."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


def case(binary, tui):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / 'bin').mkdir()
        (root / 'config').mkdir()
        (root / 'config/config.toml').write_text('roots = [' + json.dumps(tmp) + ']\n')
        agent = root / 'bin/claude'
        agent.write_text('#!' + sys.executable + '\nimport os, signal, time\n'
                         'signal.signal(signal.SIGINT, lambda *args: exit(0))\n'
                         'open("ready", "w").write(str(os.getpid()))\n'
                         'while True: time.sleep(1)\n')
        agent.chmod(0o755)
        (root / 'bin/tmux').write_text('#!/bin/sh\nexit 1\n')
        (root / 'bin/tmux').chmod(0o755)
        env = dict(os.environ, HOME=tmp, TERM='xterm-256color', PS1='JOB_READY> ',
                   PATH=str(root / 'bin') + os.pathsep + os.environ['PATH'],
                   SBOARD_CONFIG_DIR=str(root / 'config'), SBOARD_STATE_DIR=str(root / 'state'),
                   CLAUDE_CONFIG_DIR=str(root / 'claude'), CODEX_HOME=str(root / 'codex'),
                   GROK_HOME=str(root / 'grok'), OPENCODE_DB=str(root / 'opencode.db'))
        env.pop('TMUX', None)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))

        def setup():
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        shell = subprocess.Popen(['/bin/bash', '--noprofile', '--norc', '-i'], env=env,
                                 stdin=slave, stdout=slave, stderr=slave, preexec_fn=setup)
        output = b''
        board = child = None

        def wait_for(predicate, description):
            nonlocal output
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if select.select([master], [], [], .02)[0]:
                    try:
                        output += os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                if predicate():
                    return
            raise AssertionError(f'{description}: {output[-1000:]!r}')

        try:
            wait_for(lambda: b'JOB_READY>' in output, 'initial shell prompt')
            args = [binary] + ([] if tui else ['new', tmp])
            wrapper = ('import os; open(' + repr(str(root / 'board')) +
                       ', "w").write(str(os.getpid())); os.execv(' + repr(binary) +
                       ', ' + repr(args) + ')')
            os.write(master, (shlex.quote(sys.executable) + ' -c ' + shlex.quote(wrapper) + '\n').encode())
            wait_for(lambda: (root / 'board').exists(), 'board pid')
            board = int((root / 'board').read_text())
            if tui:
                wait_for(lambda: b'1 projects' in output, 'board tree')
                os.write(master, b'n')
                wait_for(lambda: b'Agent:' in output, 'agent picker')
                os.write(master, b'c')
            wait_for(lambda: (root / 'ready').exists(), 'agent ready')
            child = int((root / 'ready').read_text())
            assert os.tcgetpgrp(master) == child
            output = b''
            os.write(master, b'\x1a')
            wait_for(lambda: b'JOB_READY>' in output, 'Ctrl-Z must return shell prompt')
            assert os.tcgetpgrp(master) == shell.pid
            output = b''
            os.write(master, b'fg\n')
            wait_for(lambda: os.tcgetpgrp(master) == child, 'fg must restore agent foreground')
            os.write(master, b'\x03')
            if tui:
                wait_for(lambda: b'\x1b[?1049h' in output, 'board returns after child exit')
                time.sleep(.3)  # Existing handoff input-discard window.
                os.write(master, b'q')
            wait_for(lambda: b'JOB_READY>' in output, 'job completes after fg and Ctrl-C')
            try:
                os.kill(child, 0)
                raise AssertionError('agent survived normal completion')
            except ProcessLookupError:
                pass
            os.write(master, b'exit\n')
            # macOS can wait for PTY output to drain before finishing session exit.
            wait_for(lambda: shell.poll() is not None, 'shell exits after exit command')
            assert shell.returncode == 0
            print(('TUI' if tui else 'CLI') + ': Ctrl-Z returns shell; fg resumes agent; Ctrl-C returns normally')
        finally:
            # Release terminal drain before waiting, including failed-probe cleanup.
            os.close(master)
            os.close(slave)
            for pid in [child, board, shell.pid]:
                if pid:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            shell.wait(timeout=5)
            for pid in [child, board]:
                if pid:
                    deadline = time.monotonic() + 2
                    while True:
                        try:
                            if os.waitpid(pid, os.WNOHANG)[0]:
                                break
                        except ChildProcessError:
                            break
                        assert time.monotonic() < deadline, 'probe process was not reaped'
                        time.sleep(.01)


if __name__ == '__main__':
    if sys.platform == 'linux':
        import ctypes
        assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
    binary = str(Path(sys.argv[1]).resolve())
    case(binary, False)
    case(binary, True)
