"""Real tmux checks use only unique private sockets, scratch stores and fake agents."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

BINARY = str(Path(sys.argv[1]).resolve())
TMUX = shutil.which('tmux')
if not TMUX:
    print('private tmux checks skipped: tmux not installed')
    sys.exit(0)


def wait_for(predicate, message, seconds=2):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError(message)


def case(mode):
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp).resolve()
        bindir = root / 'bin'
        bindir.mkdir()
        for directory in ('claude/projects', 'codex/sessions', 'config', 'state'):
            (root / directory).mkdir(parents=True)
        env = dict(os.environ, HOME=str(root), TERM='xterm-256color',
                   PATH=str(bindir) + os.pathsep + os.environ['PATH'],
                   CLAUDE_CONFIG_DIR=str(root / 'claude'), CODEX_HOME=str(root / 'codex'),
                   SBOARD_CONFIG_DIR=str(root / 'config'), SBOARD_STATE_DIR=str(root / 'state'),
                   RECORD_CWD=str(root / 'cwd'), RECORD_ARGS=str(root / 'args'),
                   RECORD_STORES=str(root / 'stores'))
        env.pop('TMUX', None)
        config = root / 'tmux.conf'
        config.write_text('set -g default-shell /bin/sh\nset -g exit-empty off\n')
        socket = 'sboard-test-' + uuid.uuid4().hex
        tmux = [TMUX, '-f', str(config), '-L', socket]
        def run_tmux(*args):
            return subprocess.run(tmux + list(args), cwd=root, env=env,
                                  capture_output=True, timeout=5, check=True)
        def board(*args):
            return subprocess.run([BINARY] + list(args), cwd=root, env=env,
                                  capture_output=True, timeout=5)
        pid = None
        try:
            run_tmux('new-session', '-d', '-s', 'test', '-c', str(root), 'sleep', '60')
            tmux_env = run_tmux('display', '-p', '#{socket_path},#{pid},0').stdout.decode().strip()
            if mode == 'security':
                env['TMUX'] = tmux_env
                # The server's old store settings and executable search path must not win.
                for directory in ('current-claude/projects', 'current-codex/sessions', 'rogue'):
                    (root / directory).mkdir(parents=True)
                env['CLAUDE_CONFIG_DIR'] = str(root / 'current-claude')
                env['CODEX_HOME'] = str(root / 'current-codex')
                rogue = root / 'rogue/claude'
                rogue.write_text('#!/bin/sh\necho wrong > "' + str(root / 'WRONG') + '"\n')
                rogue.chmod(0o755)
                run_tmux('set-environment', '-g', 'PATH', str(root / 'rogue') + ':/usr/bin:/bin')
                (root / 'config/config.toml').write_text('[tmux]\nsocket = "different-server"\n')
                agent = bindir / 'claude'
                agent.write_text('#!/bin/sh\npwd -P > "$RECORD_CWD"\nprintf "%s\\n" "$@" > "$RECORD_ARGS"\nprintf "%s\\n" "$CLAUDE_CONFIG_DIR" "$CODEX_HOME" > "$RECORD_STORES"\n')
                agent.chmod(0o755)
                project = root / 'evil#(touch PWNED)'
                project.mkdir()
                result = board('new', str(project))
                assert result.returncode == 0, result.stderr
                wait_for(lambda: (root / 'stores').exists(), 'fake agent not launched')
                assert (root / 'cwd').read_text().removesuffix('\n') == str(project), 'format-expanded cwd'
                assert (root / 'args').read_text() == '--\n', 'tmux did not receive split command argv'
                assert (root / 'stores').read_text().splitlines() == [env['CLAUDE_CONFIG_DIR'], env['CODEX_HOME']]
                assert not (root / 'PWNED').exists()
                assert not (root / 'WRONG').exists()
                for args in (['init', '-q'], ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-qm', 'initial']):
                    subprocess.run(['git'] + args, cwd=project, capture_output=True, check=True)
                (root / 'stores').unlink()
                result = board('new', str(project), '--worktree', 'x#S')
                assert result.returncode == 0, result.stderr
                wait_for(lambda: (root / 'stores').exists(), 'worktree agent not launched')
                assert (root / 'cwd').read_text().removesuffix('\n') == str(root / 'evil#(touch PWNED)-wt/x#S')
                assert not (root / 'PWNED').exists()
                assert not (root / 'WRONG').exists()
            else:
                wrapper = root / 'board'
                wrapper.write_text('#!/bin/sh\necho $$ > "' + str(root / 'pid') + '"\nexec "' + BINARY + '"\n')
                wrapper.chmod(0o755)
                run_tmux('new-window', '-c', str(root), str(wrapper))
                wait_for(lambda: (root / 'pid').exists(), 'board did not launch')
                pid = int((root / 'pid').read_text())
                # Give the event reader time to enter its poll before closing the pty.
                time.sleep(.25)
                run_tmux('kill-server')
                def gone():
                    status = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True).stdout.decode().strip()
                    return not status or status.startswith('Z')
                wait_for(gone, 'sboard survived terminal closure for more than 2 seconds')
            print(mode + ': private tmux regression passed')
        finally:
            subprocess.run(tmux + ['kill-server'], capture_output=True, env=env)
            if pid is not None:
                status = subprocess.run(['ps', '-p', str(pid), '-o', 'comm='], capture_output=True).stdout.decode()
                if 'sboard' in status:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass


for mode in ([sys.argv[2]] if len(sys.argv) > 2 else ['security', 'close']):
    case(mode)
