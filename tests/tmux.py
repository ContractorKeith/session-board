"""Real tmux checks use only unique private sockets, scratch stores and fake agents."""
import json
from datetime import datetime, timezone
import os
import pty
import shlex
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pty_probe import deadline, drain, reap

BINARY = str(Path(sys.argv[1]).resolve())
TMUX = shutil.which('tmux')
if not TMUX:
    print('private tmux checks skipped: tmux not installed')
    sys.exit(0)


def wait_for(predicate, message, seconds=10):
    global stage
    stage = message
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError(message)


def case(mode):
    global stage
    stage = 'setup'
    with deadline(lambda: f'tmux.py::{mode}: waiting for {stage}'), tempfile.TemporaryDirectory() as temp:
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
                   RECORD_STORES=str(root / 'stores'), RECORD_DONE=str(root / 'done'))
        env.pop('TMUX', None)
        config = root / 'tmux.conf'
        config.write_text('set -g default-shell /bin/sh\nset -g exit-empty off\n')
        socket = 'sboard-test-' + uuid.uuid4().hex
        # Unicode pane-path framing must not depend on the runner's locale.
        tmux = [TMUX, '-u', '-f', str(config), '-L', socket]
        def run_tmux(*args):
            return subprocess.run(tmux + list(args), cwd=root, env=env,
                                  capture_output=True, timeout=5, check=True)
        def board(*args):
            return subprocess.run([BINARY] + list(args), cwd=root, env=env,
                                  capture_output=True, timeout=5)
        pid = None
        client = None
        master = None
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
                agent.write_text('#!/bin/sh\npwd -P > "$RECORD_CWD"\nprintf "%s\\n" "$@" > "$RECORD_ARGS"\nprintf "%s\\n" "$CLAUDE_CONFIG_DIR" "$CODEX_HOME" > "$RECORD_STORES"\n: > "$RECORD_DONE"\n')
                agent.chmod(0o755)
                project = root / 'evil#(touch PWNED)'
                project.mkdir()
                result = board('new', str(project))
                assert result.returncode == 0, result.stderr
                wait_for(lambda: (root / 'done').exists(), 'fake agent not launched')
                assert (root / 'cwd').read_text().removesuffix('\n') == str(project), 'format-expanded cwd'
                assert (root / 'args').read_text() == '--\n', 'tmux did not receive split command argv'
                assert (root / 'stores').read_text().splitlines() == [env['CLAUDE_CONFIG_DIR'], env['CODEX_HOME']]
                assert not (root / 'PWNED').exists()
                assert not (root / 'WRONG').exists()
                for args in (['init', '-q'], ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--allow-empty', '-qm', 'initial']):
                    subprocess.run(['git'] + args, cwd=project, capture_output=True, check=True)
                (root / 'stores').unlink()
                (root / 'done').unlink()
                result = board('new', str(project), '--worktree', 'x#S')
                assert result.returncode == 0, result.stderr
                wait_for(lambda: (root / 'done').exists(), 'worktree agent not launched')
                assert (root / 'cwd').read_text().removesuffix('\n') == str(root / 'evil#(touch PWNED)-wt/x#S')
                assert not (root / 'PWNED').exists()
                assert not (root / 'WRONG').exists()
            elif mode in ('hash-jump', 'jump-race'):
                victim = root / 'victim-界é'
                victim.mkdir()
                target = 'a#b' if mode == 'hash-jump' else 'race'
                run_tmux('new-session', '-d', '-s', target.replace('#', '##'), '-c', str(victim), 'sleep', '60')
                # tmux can reply before the new pane child has entered its requested cwd.
                wait_for(lambda: run_tmux('list-panes', '-t', '=' + target, '-F',
                    '#{pane_current_path}').stdout.decode().strip() == str(victim),
                    'target pane did not enter the fixture cwd')
                capture = run_tmux('list-panes', '-t', '=' + target, '-F',
                    '#{n:session_name}:#{session_name}#{n:window_index}:#{window_index}#{n:pane_index}:#{pane_index}6:claude9:999999999#{n:pane_current_path}:#{pane_current_path}').stdout
                (root / 'panes').write_bytes(capture)
                rows = [json.loads(line) for line in Path('fixtures/claude_code/project/done.jsonl').read_text().splitlines()]
                for row in rows:
                    if 'cwd' in row: row['cwd'] = str(victim)
                    if 'timestamp' in row: row['timestamp'] = datetime.now(timezone.utc).isoformat()
                (root / 'claude/projects/project').mkdir()
                (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
                agent = bindir / 'claude'
                agent.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$RECORD_ARGS"\n')
                agent.chmod(0o755)
                shim = bindir / 'tmux'
                shim.write_text("""#!/bin/sh
socket="$2"
shift 2
if [ "$1" = list-panes ]; then cat "$PANES"; exit; fi
if [ "$1" = display ] && [ "$3" = -t ] && [ "$RACE" = 1 ] && [ ! -e "$RACE_MARK" ]; then
    "$REAL_TMUX" -S "$socket" "$@"
    result=$?
    "$REAL_TMUX" -S "$socket" kill-session -t "=$TARGET"
    : > "$RACE_MARK"
    exit "$result"
fi
exec "$REAL_TMUX" -S "$socket" "$@"
""")
                shim.chmod(0o755)
                for key, value in dict(REAL_TMUX=TMUX, PANES=str(root / 'panes'), TARGET=target,
                                       RACE='1' if mode == 'jump-race' else '0', RACE_MARK=str(root / 'raced')).items():
                    run_tmux('set-environment', '-g', key, value)
                client, master = pty.fork()
                if client == 0:
                    os.execvpe(TMUX, tmux + ['attach', '-t', '=test'], env)
                def connected():
                    drain(master)
                    return bool(run_tmux('list-clients', '-F', '#{client_session}').stdout.strip())
                wait_for(connected, 'private client did not attach')
                wrapper = root / 'open'
                wrapper.write_text('#!/bin/sh\n' + shlex.join([BINARY, 'open', 'claude_code:done']) +
                    '\nresult=$?\nprintf "%s\\n" "$result" > ' + shlex.quote(str(root / 'exit.tmp')) + '\nmv ' + shlex.quote(str(root / 'exit.tmp')) + ' ' + shlex.quote(str(root / 'exit')) + '\nsleep 60\n')
                wrapper.chmod(0o755)
                run_tmux('new-window', '-t', '=test', '-c', str(root), str(wrapper))
                wait_for(lambda: (root / 'exit').exists(), 'open did not return')
                assert (root / 'exit').read_text().strip() == '0'
                if mode == 'hash-jump':
                    try:
                        wait_for(lambda: run_tmux('list-clients', '-F', '#{client_session}').stdout.strip() == b'a#b',
                                 'private client did not switch to the literal hash session')
                    except AssertionError as error:
                        clients = run_tmux('list-clients', '-F', '#{client_session}').stdout
                        resumed = (root / 'args').read_bytes() if (root / 'args').exists() else None
                        raise AssertionError(f'{error}; clients={clients!r}; resume argv={resumed!r}') from error
                    assert not (root / 'args').exists(), 'literal hash target resumed instead of jumping'
                else:
                    assert (root / 'raced').exists(), 'shim never removed the target'
                    assert (root / 'args').read_text() == '--resume=done\n', 'jump failure did not resume'
            elif mode == 'injection':
                env['TMUX'] = tmux_env
                victim = root / 'victim'
                victim.mkdir()
                evil = root / ('inject\x1e\nbase\x1f1\x1f1\x1fclaude\x1f999999999\x1f' + str(victim))
                evil.mkdir(parents=True)
                run_tmux('new-window', '-c', str(evil), 'sleep', '30')
                rows = [json.loads(line) for line in Path('fixtures/claude_code/project/done.jsonl').read_text().splitlines()]
                for row in rows:
                    if 'cwd' in row: row['cwd'] = str(victim)
                    if 'timestamp' in row: row['timestamp'] = datetime.now(timezone.utc).isoformat()
                (root / 'claude/projects/project').mkdir()
                (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
                result = board('list', '--all', '--json')
                assert result.returncode == 0, result.stderr
                sessions = [session for project in json.loads(result.stdout) for session in project['sessions']]
                assert len(sessions) == 1
                assert sessions[0]['pane'] is None, 'cwd forged a pane: ' + repr(sessions[0]['pane'])
            else:
                wrapper = root / 'board'
                wrapper.write_text('#!/bin/sh\necho $$ > ' + shlex.quote(str(root / 'pid.tmp')) + '\nmv ' + shlex.quote(str(root / 'pid.tmp')) + ' ' + shlex.quote(str(root / 'pid')) + '\nexec ' + shlex.quote(BINARY) + '\n')
                wrapper.chmod(0o755)
                pane = run_tmux('new-window', '-P', '-F', '#{pane_id}', '-c', str(root), str(wrapper)).stdout.decode().strip()
                wait_for(lambda: (root / 'pid').exists(), 'board did not launch')
                pid = int((root / 'pid').read_text())
                # A rendered frame acknowledges that sboard entered its terminal event loop.
                wait_for(lambda: b'Projects' in run_tmux('capture-pane', '-p', '-t', pane).stdout,
                         'board never rendered its first frame')
                run_tmux('kill-server')
                def gone():
                    status = subprocess.run(['ps', '-p', str(pid), '-o', 'stat='], capture_output=True, timeout=5).stdout.decode().strip()
                    return not status or status.startswith('Z')
                wait_for(gone, 'sboard survived terminal closure for more than 2 seconds', seconds=2)
            print(mode + ': private tmux regression passed')
        finally:
            subprocess.run(tmux + ['kill-server'], capture_output=True, env=env, timeout=5)
            if client is not None:
                reap(client, f'tmux.py::{mode}: client cleanup')
                os.close(master)
            if pid is not None:
                status = subprocess.run(['ps', '-p', str(pid), '-o', 'comm='], capture_output=True, timeout=5).stdout.decode()
                if 'sboard' in status:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass


for mode in ([sys.argv[2]] if len(sys.argv) > 2 else ['security', 'close']):
    case(mode)
