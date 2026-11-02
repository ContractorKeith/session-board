"""Drive script(1)'s PTY; all stores/config/state are scratch inputs."""
import fcntl
import json
from datetime import datetime, timezone, timedelta
import os
from pathlib import Path
import pty
import select
import re
import unicodedata
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import time


def screen(data):
    # Reconstruct cursor-addressed diff output without an external terminal emulator.
    grid = [[' '] * 100 for _ in range(30)]
    row = col = 0
    # Rebuilt from the full transcript each call; wait for a partial CSI's next read.
    text = re.sub(r'\x1b\[[?0-9;]*$', '', data.decode('utf-8', errors='ignore'))
    chunks = re.split(r'(\x1b\[[?0-9;]*[A-Za-z])', text)
    for chunk in chunks:
        if chunk.startswith('\x1b['):
            match = re.fullmatch(r'\x1b\[([?0-9;]*)([A-Za-z])', chunk)
            if match is None: continue  # The next PTY read may complete this escape.
            params, op = match.groups()
            values = [int(x or 1) for x in params.split(';')] if '?' not in params else []
            if op in ('H', 'f'):
                row, col = (values + [1, 1])[:2]
                row -= 1; col -= 1
            elif op == 'J' and params == '2':
                grid = [[' '] * 100 for _ in range(30)]
            continue
        for char in chunk:
            if char == '\r': col = 0
            elif char == '\n':
                if row == 29: grid.pop(0); grid.append([' '] * 100)
                else: row += 1
            elif char.isprintable() and not unicodedata.combining(char):
                if 0 <= row < 30 and 0 <= col < 100: grid[row][col] = char
                col += 2 if unicodedata.east_asian_width(char) in ('W', 'F') else 1
    return '\n'.join(''.join(line) for line in grid)


def case(mode):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / 'claude/projects/project').mkdir(parents=True)
        (root / 'codex/sessions').mkdir(parents=True)
        rows = [json.loads(line) for line in Path('fixtures/claude_code/project/done.jsonl').read_text().splitlines()]
        for row in rows:
            if 'timestamp' in row: row['timestamp'] = (datetime.now(timezone.utc) - timedelta(seconds=60)).isoformat().replace('+00:00', 'Z')
        (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        env = dict(os.environ, TERM='xterm-256color',
                   CLAUDE_CONFIG_DIR=str(root / 'claude'), CODEX_HOME=str(root / 'codex'),
                   SBOARD_STATE_DIR=str(root / 'state'), SBOARD_CONFIG_DIR=str(root / 'config'))
        if mode == 'unwatched': (root / 'codex/sessions').rmdir()
        bindir = root / 'bin'
        bindir.mkdir()
        (bindir / 'tmux').write_text('#!/bin/sh\nexit 1\n')
        (bindir / 'tmux').chmod(0o755)
        (bindir / 'claude').write_text('#!/bin/sh\nstty -a > "$TERMINAL_FLAGS"\nprintf "agent returned\\n"\n')
        (bindir / 'claude').chmod(0o755)
        if mode in ('termios', 'cli-termios'):
            (bindir / 'claude').write_text('#!/bin/sh\nstty -a >> "$TERMINAL_FLAGS"\necho END >> "$TERMINAL_FLAGS"\nstty raw -echo\n')
        if mode in ('child-int', 'cli-int', 'cli-open-int'):
            (bindir / 'claude').write_text('#!/bin/sh\ntrap "echo trapped > \"$INT_FILE\"" INT\n(trap "" INT; echo ready > "$READY_FILE"; sleep 3)\necho finished > "$FINISHED_FILE"\n')
        if mode in ('stale-pane', 'socket-mismatch', 'vanished-pane', 'jump-failure', 'attach-failure'):
            (bindir / 'tmux').write_text('#!/bin/sh\nif [ "$1" = -S ] || [ "$1" = -L ]; then shift 2; fi\nif [ "$1" = list-panes ]; then [ -f "$PANES" ] && cat "$PANES"; elif [ "$1" = display ]; then echo %0; else exit 0; fi\n')
            env['PANES'] = str(root / 'panes')
        if mode in ('stale-pane', 'vanished-pane', 'jump-failure', 'attach-failure'):
            (root / 'panes').write_text(''.join(str(len(v.encode())) + ':' + v for v in ['dev', '2', '1', 'claude', '999999999', str(root)]) + '\n')
        if mode == 'socket-mismatch':
            (root / 'config').mkdir()
            (root / 'config/config.toml').write_text('[tmux]\nsocket = "other"\n')
        if mode == 'vanished-pane':
            (bindir / 'tmux').write_text('#!/bin/sh\nif [ "$1" = -S ] || [ "$1" = -L ]; then shift 2; fi\nif [ "$1" = list-panes ]; then cat "$PANES"; elif [ "$1" = display ]; then [ "$3" = -t ] && exit 1; echo %0; else exit 0; fi\n')
        if mode == 'jump-failure':
            (bindir / 'tmux').write_text('#!/bin/sh\nif [ "$1" = -S ] || [ "$1" = -L ]; then shift 2; fi\nif [ "$1" = list-panes ]; then cat "$PANES"; elif [ "$1" = display ]; then echo %0; elif [ "$1" = switch-client ]; then echo pane-gone >&2; exit 1; else exit 0; fi\n')
        if mode == 'attach-failure':
            (bindir / 'tmux').write_text('#!/bin/sh\nif [ "$1" = -S ] || [ "$1" = -L ]; then shift 2; fi\nif [ "$1" = list-panes ]; then cat "$PANES"; elif [ "$1" = display ]; then echo %0; elif [ "$1" = attach ]; then echo attach-denied >&2; exit 1; else exit 0; fi\n')
        env['READY_FILE'] = str(root / 'ready')
        env['FINISHED_FILE'] = str(root / 'finished')
        env['INT_FILE'] = str(root / 'int')
        env['PATH'] = str(bindir) + os.pathsep + env['PATH']
        env['TERMINAL_FLAGS'] = str(root / 'flags')
        env.pop('TMUX', None)
        if mode in ('socket-mismatch', 'vanished-pane', 'jump-failure'): env['TMUX'] = '/fake/live,1,0'
        if mode in ('resume', 'launch', 'termios', 'child-int', 'cli-int', 'cli-open-int', 'message', 'stale-pane', 'socket-mismatch', 'vanished-pane', 'jump-failure', 'attach-failure'):
            for row in rows:
                if 'cwd' in row: row['cwd'] = str(root)
            (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        env.pop('NO_COLOR', None)
        if mode.startswith('color-'): env['NO_COLOR'] = '1' if mode == 'color-off' else ''
        env.pop('SBOARD_SCAN_PROBE', None)
        env.pop('SBOARD_TERMINAL_PROBE', None)
        command = [BINARY]
        if mode in ('cli-int', 'cli-termios'):
            command += ['new', str(root)]
        if mode == 'cli-open-int':
            command += ['open', 'claude_code:done']
        if mode in ('hook', 'error'):
            command = [PROBE, '--ignored', '--exact', 'terminal_probe', '--nocapture']
            env['SBOARD_TERMINAL_PROBE'] = mode
        if mode == 'dumb': env['TERM'] = 'dumb'
        if mode == 'scan-panic':
            env['SBOARD_SCAN_PROBE'] = 'panic-once'
        pidfile = root / 'pid'
        text = 'echo $$ > ' + shlex.quote(str(pidfile)) + '; exec ' + shlex.join(command)
        if mode in ('termios', 'cli-termios'):
            text = 'echo $$ > ' + shlex.quote(str(pidfile)) + '; ' + shlex.join(command) + '; result=$?; stty -a > ' + shlex.quote(str(root / 'exit-flags')) + '; exit "$result"'
        if mode == 'stdin': text += ' < /dev/null'
        typescript = root / 'typescript'
        if sys.platform == 'darwin':
            args = ['script', '-q', str(typescript), 'sh', '-c', text]
        else:
            args = ['script', '-qefc', text, str(typescript)]
        master, slave = pty.openpty()
        fcntl.ioctl(slave, __import__('termios').TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        proc = subprocess.Popen(args, env=env, stdin=slave, stdout=slave, stderr=slave)
        output = b''
        def pump():
            nonlocal output
            if select.select([master], [], [], .02)[0]:
                output += os.read(master, 65536)
        def wait_for(predicate, description):
            end = time.monotonic() + 8
            while time.monotonic() < end:
                pump()
                if predicate():
                    return
                if proc.poll() is not None:
                    break
            raise AssertionError(f'{mode}: {description}; return={proc.poll()}; output={output[-1200:]!r}')
        try:
            if mode in ('dumb', 'stdin'):
                wait_for(lambda: proc.poll() is not None, 'refusal did not exit')
                data = typescript.read_bytes()
                assert b'\x1b[?1049h' not in data
                assert b'sboard: interactive terminal required; run sboard list for script output.' in data
                print(f'{mode}: refused before alternate screen')
                return
            if mode == 'cli-termios':
                wait_for(lambda: proc.poll() is not None, 'CLI did not return after raw child')
                flags = (root / 'exit-flags').read_text()
                for flag in ('icanon', 'echo', 'icrnl'):
                    assert re.search(r'(?<!-)\b' + flag + r'\b', flags), flags
                return
            if mode in ('cli-int', 'cli-open-int'):
                wait_for(lambda: (root / 'ready').exists(), 'CLI child not ready')
                os.write(master, b'\x03')
                until = time.monotonic() + .4
                while time.monotonic() < until: pump()
                assert proc.poll() is None, 'CLI parent died on child Ctrl-C'
                wait_for(lambda: proc.poll() is not None, 'CLI failed to wait for child')
                assert (root / 'finished').exists(), 'CLI orphaned child'
                return
            wait_for(lambda: b'Projects' in output, 'no initial frame')
            pid = int(pidfile.read_text())
            if mode == 'scan-panic':
                wait_for(lambda: 'scan: panicked' in screen(output), 'worker panic was hidden or deadlocked')
                assert 'Projects' in screen(output) and '? help' in screen(output)
                assert b'\x1b[?1049l' not in output, 'worker panic restored the live terminal'
                os.write(master, b'r')
                wait_for(lambda: '1 projects' in screen(output), 'scan did not recover on r')
                os.write(master, b'q')
            elif mode in ('TERM', 'INT', 'HUP'):
                os.kill(pid, getattr(signal, 'SIG' + mode))
            elif mode == 'unwatched':
                wait_for(lambda: 'unwatched: codex' in screen(output), 'missing root not reported')
                (root / 'codex/sessions').mkdir()
                wait_for(lambda: 'unwatched:' not in screen(output) and '1 projects' in screen(output), 'reattached root remained unwatched')
                os.write(master, b'q')
            elif mode == 'socket-mismatch':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'n')
                wait_for(lambda: 'Agent: c claude_code' in screen(output), 'no prompt')
                os.write(master, b'c')
                wait_for(lambda: 'using TMUX socket' in screen(output), 'socket mismatch not reported')
                os.write(master, b'q')
            elif mode in ('stale-pane', 'vanished-pane', 'jump-failure', 'attach-failure'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'j')
                wait_for(lambda: 'Pane: dev:2.1' in screen(output), 'initial pane not assigned')
                if mode == 'stale-pane': (root / 'panes').unlink()
                os.write(master, b'\r')
                wait_for(lambda: (root / 'flags').exists() and 'resumed ' in screen(output) and output.count(b'\x1b[?1049h') >= 2, 'Enter did not suspend around stale-pane fallback')
                if mode == 'jump-failure': assert 'pane-gone' in screen(output), 'tmux failure message was lost'
                if mode == 'attach-failure': assert 'attach-denied' in screen(output), 'tmux attach message was lost'
                os.write(master, b'q')
            elif mode in ('termios', 'child-int', 'message'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                for child in range(2 if mode == 'termios' else 1):
                    os.write(master, b'n')
                    wait_for(lambda: 'Agent: c claude_code' in screen(output), 'no prompt')
                    before = output.count(b'\x1b[?1049h')
                    os.write(master, b'c')
                    if mode == 'child-int':
                        wait_for(lambda: (root / 'ready').exists(), 'child not ready')
                        os.write(master, b'\x03')
                    wait_for(lambda: output.count(b'\x1b[?1049h') > before and 'launched ' in screen(output), 'board did not return after child')
                if mode in ('termios', 'cli-termios'):
                    for flags in (root / 'flags').read_text().split('END')[:-1]:
                        for flag in ('icanon', 'echo', 'icrnl'):
                            assert re.search(r'(?<!-)\b' + flag + r'\b', flags), flags
                if mode == 'child-int':
                    assert (root / 'int').exists(), 'child never received Ctrl-C'
                    assert proc.poll() is None, 'board exited on child Ctrl-C'
                if mode == 'message':
                    (root / 'claude/projects/project/session.jsonl').write_text((root / 'claude/projects/project/session.jsonl').read_text() + '\n')
                    until = time.monotonic() + 3.5
                    while time.monotonic() < until: pump()
                    assert 'launched ' in screen(output), 'scan overwrote outcome'
                    os.write(master, b'j')
                    wait_for(lambda: 'launched ' not in screen(output), 'next key did not clear outcome')
                os.write(master, b'q')
            elif mode in ('resume', 'launch'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                if mode == 'resume':
                    os.write(master, b'j\r')
                else:
                    os.write(master, b'n')
                    wait_for(lambda: 'Agent: c claude_code' in screen(output), 'no agent prompt')
                    print(screen(output))
                    os.write(master, b'c')
                wait_for(lambda: (root / 'flags').exists() and output.count(b'\x1b[?1049h') >= 2, 'agent did not return to alternate screen')
                flags = (root / 'flags').read_text()
                assert re.search(r'(?<!-)\bicanon\b', flags), flags
                assert re.search(r'(?<!-)\becho\b', flags), flags
                wait_for(lambda: ('resumed ' if mode == 'resume' else 'launched ') in screen(output), 'outcome missing after return')
                os.write(master, b'q')
            elif mode in ('q', 'idle', 'color-empty', 'color-off'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                if mode == 'idle':
                    initial = output.count(b'\x1b[?25l')
                    until = time.monotonic() + 2.2
                    while time.monotonic() < until: pump()
                    assert output.count(b'\x1b[?25l') - initial <= 4, 'redraws exceed the age tick'
                if mode.startswith('color-'):
                    colored = any(any(30 <= int(n) <= 37 or int(n) in (38, 48) or 90 <= int(n) <= 107 for n in params.split(b';') if n.isdigit()) for params in re.findall(rb'\x1b\[([0-9;]*)m', output))
                    assert colored == (mode == 'color-empty'), (mode, colored)
                os.write(master, b'q')
            wait_for(lambda: proc.poll() is not None, 'process failed to exit')
            while select.select([master], [], [], .02)[0]:
                output += os.read(master, 65536)
            if mode in ('termios', 'cli-termios'):
                flags = (root / 'exit-flags').read_text()
                for flag in ('icanon', 'echo', 'icrnl'):
                    assert re.search(r'(?<!-)\b' + flag + r'\b', flags), flags
            data = typescript.read_bytes()
            frame = data.index(b'Projects')
            assert b'\x1b[?1049l' in data[frame:], f'{mode}: alternate screen not restored'
            assert b'\x1b[?25h' in data[frame:], f'{mode}: cursor not restored'
            if sys.platform != 'darwin':
                expected = 128 + getattr(signal, 'SIG' + mode) if mode in ('TERM','INT','HUP') else (86 if mode == 'hook' else 0)
                assert proc.returncode == expected, (mode, proc.returncode, expected)
            print(f'{mode}: frame, exit, alternate-screen and cursor restoration passed')
        finally:
            if proc.poll() is None:
                if pidfile.exists():
                    try: os.kill(int(pidfile.read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass
                proc.kill()
                proc.wait()
            os.close(master)
            os.close(slave)

if __name__ == '__main__':
    BINARY = str(Path(sys.argv[1]).resolve())
    PROBE = str(Path(sys.argv[2]).resolve()) if len(sys.argv) > 2 else None
    for mode in ([os.environ['SBOARD_PTY_CASE']] if 'SBOARD_PTY_CASE' in os.environ else ['TERM'] if PROBE is None else ['TERM', 'INT', 'HUP', 'q', 'scan-panic', 'error', 'hook', 'dumb', 'stdin', 'idle', 'color-empty', 'color-off', 'unwatched', 'resume', 'launch', 'termios', 'child-int', 'cli-int', 'cli-open-int', 'message', 'stale-pane', 'socket-mismatch', 'vanished-pane', 'jump-failure', 'attach-failure', 'cli-termios']):
        case(mode)
