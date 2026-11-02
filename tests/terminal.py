"""Drive script(1)'s PTY; all stores/config/state are scratch inputs."""
import fcntl
import json
from datetime import datetime, timezone, timedelta
import os
from pathlib import Path
import pty
import re
import unicodedata
import shlex
import signal
import struct
import subprocess
import sys
import tempfile
import time
import tomllib
import threading
from pty_probe import deadline, drain, read_ready


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
    stage = 'setup'
    with deadline(lambda: f'script_pty_terminal_hygiene_and_worker_recovery [{mode}]: waiting for {stage}'), tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / 'claude/projects/project').mkdir(parents=True)
        (root / 'codex/sessions').mkdir(parents=True)
        fixture = 'waiting' if mode in ('attention-resume', 'attention-fresh-pane', 'attention-collapsed-read', 'attention-read-in-child') else 'done'
        rows = [json.loads(line) for line in Path(f'fixtures/claude_code/project/{fixture}.jsonl').read_text().splitlines()]
        for row in rows:
            if 'timestamp' in row: row['timestamp'] = (datetime.now(timezone.utc) - timedelta(seconds=60)).isoformat().replace('+00:00', 'Z')
        (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        env = dict(os.environ, HOME=str(root), TERM=os.environ.get('SBOARD_PTY_TERM', 'xterm-256color'),
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
        if mode in ('typeahead', 'paste-handoff'):
            (bindir / 'claude').write_text('#!/bin/sh\necho ready > "$READY_FILE"\nIFS= read -r line\nprintf "%s" "$line" > "$AGENT_INPUT"\necho done > "$FINISHED_FILE"\n')
            env['AGENT_INPUT'] = str(root / 'agent-input')
            for row in rows:
                if 'cwd' in row: row['cwd'] = str(root)
            (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        if mode in ('attention-resume', 'attention-fresh-pane', 'attention-collapsed-read'):
            (bindir / 'claude').write_text('#!/bin/sh\nstty -a > "$TERMINAL_FLAGS"\nprintf "%s\\n" "$@" > "$ATTENTION_ARGS"\nprintf "agent returned\\n"\n')
            env['ATTENTION_ARGS'] = str(root / 'attention-args')
        if mode == 'attention-fresh-pane':
            env['PANES'] = str(root / 'panes')
            (bindir / 'tmux').write_text('#!/bin/sh\nif [ "$1" = -S ] || [ "$1" = -L ]; then shift 2; fi\nif [ "$1" = list-panes ]; then [ -f "$PANES" ] && cat "$PANES"; elif [ "$1" = display ]; then echo %0; else exit 0; fi\n')
        if mode in ('attention-read-in-child', 'tree-read-in-child'):
            (root / 'state').mkdir()
            (root / 'state/state.toml').write_text('first_seen = 0\n')
            (bindir / 'claude').write_text('#!/bin/sh\necho ready > "$READY_FILE"\nIFS= read -r release\necho done > "$FINISHED_FILE"\n')
            for row in rows:
                if 'cwd' in row: row['cwd'] = str(root)
            (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
        if mode == 'duplicate-open':
            (root / 'claude/projects/project/session.jsonl').unlink()
            for index, name in enumerate(('first', 'second')):
                cwd = root / name
                cwd.mkdir()
                duplicate = [json.loads(line) for line in Path('fixtures/claude_code/project/waiting.jsonl').read_text().splitlines()]
                for row in duplicate:
                    row.update(sessionId='duplicate', cwd=str(cwd), timestamp=(datetime.now(timezone.utc) - timedelta(seconds=60 + index * 10)).isoformat().replace('+00:00', 'Z'))
                (root / ('claude/projects/project/' + name + '.jsonl')).write_text(''.join(json.dumps(row) + '\n' for row in duplicate))
            env['OPEN_CWD'] = str(root / 'open-cwd')
            (bindir / 'claude').write_text('#!/bin/sh\npwd -P > "$OPEN_CWD"\n')
        env['READY_FILE'] = str(root / 'ready')
        env['FINISHED_FILE'] = str(root / 'finished')
        env['INT_FILE'] = str(root / 'int')
        env['PATH'] = str(bindir) + os.pathsep + env['PATH']
        env['TERMINAL_FLAGS'] = str(root / 'flags')
        env.pop('TMUX', None)
        if mode in ('socket-mismatch', 'vanished-pane', 'jump-failure'): env['TMUX'] = '/fake/live,1,0'
        if mode in ('resume', 'attention-resume', 'attention-fresh-pane', 'attention-collapsed-read', 'launch', 'termios', 'child-int', 'cli-int', 'cli-open-int', 'message', 'stale-pane', 'socket-mismatch', 'vanished-pane', 'jump-failure', 'attach-failure'):
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
        direct = mode.startswith('escape-') or mode in ('paste-timeout', 'paste-handoff', 'paste-nested', 'decoder-keys')
        if direct:
            # Timed bytes must reach the board's PTY without script(1)'s relay.
            def controlling_terminal():
                os.setsid()
                fcntl.ioctl(slave, __import__('termios').TIOCSCTTY, 0)
            proc = subprocess.Popen(command, env=env, stdin=slave, stdout=slave, stderr=slave,
                                    preexec_fn=controlling_terminal)
            pidfile.write_text(str(proc.pid))
        else:
            proc = subprocess.Popen(args, env=env, stdin=slave, stdout=slave, stderr=slave)
        output = b''
        write_gaps = []
        def write_parts(parts, gap):
            previous = None
            for part in parts:
                if previous is not None:
                    deadline = previous + gap
                    # sleep on macOS may coalesce these sub-50 ms timing probes.
                    while time.monotonic() < deadline: pass
                sent = time.monotonic()
                if previous is not None: write_gaps.append(round((sent - previous) * 1000, 2))
                os.write(master, part)
                previous = sent
        def pump():
            nonlocal output
            output += read_ready(master, .02)
        def wait_for(predicate, description):
            nonlocal stage
            stage = description
            end = time.monotonic() + 8
            while time.monotonic() < end:
                pump()
                if predicate():
                    return
                if proc.poll() is not None:
                    break
            # The process can exit between the predicate and poll above.
            if predicate(): return
            raise AssertionError(f'{mode}: {description}; return={proc.poll()}; write_gaps_ms={write_gaps}; output={output[-1200:]!r}')
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
            elif mode in ('attention-read-in-child', 'tree-read-in-child'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan missing')
                if mode == 'attention-read-in-child':
                    os.write(master, b'h\th')
                    wait_for(lambda: 'l to expand' in screen(output), 'attention did not collapse')
                    advanced = datetime.now(timezone.utc) - timedelta(seconds=30)
                    done = [json.loads(line) for line in Path('fixtures/claude_code/project/done.jsonl').read_text().splitlines()]
                    for row in done:
                        row.update(sessionId='waiting', cwd=str(root), timestamp=advanced.isoformat().replace('+00:00', 'Z'))
                    (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in done))
                    wait_for(lambda: '•1' in screen(output), 'collapsed done activity did not become unread')
                    expected_id, expected_at = 'claude_code:waiting', int(advanced.timestamp())
                    os.write(master, b'\r')
                else:
                    expected_id = 'claude_code:done'
                    expected_at = int(datetime.fromisoformat(rows[-1]['timestamp'].replace('Z', '+00:00')).timestamp())
                    os.write(master, b'j\r')
                try:
                    wait_for(lambda: (root / 'ready').exists(), 'resume child did not start')
                    assert not (root / 'finished').exists(), 'child exited before disk check'
                    saved = tomllib.loads((root / 'state/state.toml').read_text())
                    assert saved.get('read', {}).get(expected_id, -1) >= expected_at, 'read mark was not saved before resume'
                    second = subprocess.run([BINARY, 'list', '--status', 'attention', '--json'], env=env, capture_output=True, check=True, timeout=5)
                    assert json.loads(second.stdout) == [], 'second instance still sees the resumed session unread'
                    assert not (root / 'finished').exists(), 'child exited during disk check'
                finally:
                    if (root / 'ready').exists() and not (root / 'finished').exists():
                        os.write(master, b'release\n')
                        wait_for(lambda: (root / 'finished').exists(), 'resume child did not stop')
                wait_for(lambda: 'resumed ' in screen(output), 'board did not return from child')
                os.write(master, b'q')
            elif mode == 'duplicate-open':
                wait_for(lambda: '2 projects' in screen(output), 'duplicate-id fixtures missing')
                os.write(master, b'\tj\r')
                wait_for(lambda: (root / 'open-cwd').exists() and 'resumed ' in screen(output), 'duplicate-id resume did not finish')
                assert Path((root / 'open-cwd').read_text().strip()) == (root / 'second').resolve(), 'open picked the other transcript with the same id'
                os.write(master, b'q')
            elif mode in ('attention-fresh-pane', 'attention-collapsed-read'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan missing')
                if mode == 'attention-fresh-pane':
                    (root / 'panes').write_text(''.join(str(len(v.encode())) + ':' + v for v in ['dev', '2', '1', 'claude', '999999999', str(root)]) + '\n')
                    os.write(master, b'\t\r')
                    wait_for(lambda: 'jumped to ' in screen(output) or (root / 'flags').exists(), 'open did not return')
                    assert 'jumped to ' in screen(output), 'attention Enter ignored the freshly discovered pane'
                    assert not (root / 'flags').exists(), 'attention Enter resumed instead of jumping'
                else:
                    os.write(master, b'h\th')
                    wait_for(lambda: 'l to expand' in screen(output), 'attention did not collapse')
                    advanced = datetime.now(timezone.utc) - timedelta(seconds=30)
                    for row in rows:
                        if 'timestamp' in row: row['timestamp'] = advanced.isoformat().replace('+00:00', 'Z')
                    (root / 'claude/projects/project/session.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in rows))
                    wait_for(lambda: '•1' in screen(output), 'collapsed selection update did not become unread')
                    os.write(master, b'\r')
                    wait_for(lambda: 'resumed ' in screen(output), 'collapsed attention Enter did not resume')
                    assert (root / 'attention-args').read_text().splitlines() == ['--resume=waiting']
                os.write(master, b'q')
                if mode == 'attention-collapsed-read':
                    wait_for(lambda: proc.poll() is not None, 'quit did not flush read mark')
                    saved = tomllib.loads((root / 'state/state.toml').read_text())
                    assert saved['read']['claude_code:waiting'] >= int(advanced.timestamp()), 'attention Enter did not mark the opened activity read'
            elif mode == 'escape-sequence-prefix':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                for tail in (b'\x1b[200~a\x1b[201~', b'\x1b[200~p\x1b[201~', b'\x1b[200~q\x1b[201~', b'\x1b[A', b'\x1bOA'):
                    os.write(master, b't')
                    wait_for(lambda: 'Tags:' in screen(output), 'tag prompt missing')
                    write_parts([b'\x1b', tail], .02)
                    wait_for(lambda: 'Tags:' not in screen(output), 'leading Escape did not cancel')
                    until = time.monotonic() + .15
                    while time.monotonic() < until: pump()
                    assert proc.poll() is None, 'escape-prefixed paste quit the board'
                    assert not (root / 'state/state.toml').exists(), 'escape-prefixed sequence ran a board action'
                os.write(master, b'q')
            elif mode == 'decoder-keys':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'\x1b[[A\x1b[[B\x1b[[C\x1b[[D\x1b[[E\x1b[13u\x1b[99;5u\x1b[1;5B')
                until = time.monotonic() + .15
                while time.monotonic() < until: pump()
                assert not (root / 'state/state.toml').exists(), 'ignored sequence ran a board action'
                for parts in ([b'\x1b\x1b'], [b'\x1b[', b'\x03'], [b'\xf0', b'\x03']):
                    os.write(master, b't')
                    wait_for(lambda: 'Tags:' in screen(output), 'tag prompt missing')
                    write_parts(parts, .02)
                    wait_for(lambda: 'Tags:' not in screen(output), 'Escape/control failed to cancel prompt')
                assert not (root / 'state/state.toml').exists(), 'cancelled prompt saved state'
                os.write(master, b'q')
            elif mode.startswith('escape-') or mode in ('paste-timeout', 'paste-nested'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                if mode != 'escape-alt-board':
                    os.write(master, b't')
                    wait_for(lambda: 'Tags:' in screen(output), 'tag prompt missing')
                if mode == 'escape-alone':
                    os.write(master, b'\x1b')
                    wait_for(lambda: 'Tags:' not in screen(output), 'lone Escape did not cancel')
                elif mode in ('escape-alt', 'escape-alt-board'):
                    write_parts([b'\x1b', b'p'], .02)
                    until = time.monotonic() + .2
                    while time.monotonic() < until: pump()
                    assert ('Tags:' in screen(output)) == (mode == 'escape-alt'), 'split Alt chord changed prompt'
                    assert not (root / 'state/state.toml').exists(), 'split Alt chord pinned'
                    if mode == 'escape-alt':
                        os.write(master, b'\x03')
                        wait_for(lambda: 'Tags:' not in screen(output), 'Ctrl-C did not cancel')
                elif mode == 'paste-timeout':
                    os.write(master, b'\x1b[200~foo')
                    until = time.monotonic() + .4
                    while time.monotonic() < until: pump()
                    os.write(master, b'\x03')
                    wait_for(lambda: 'Tags:' not in screen(output), 'unterminated paste swallowed Ctrl-C')
                else:
                    if mode == 'paste-nested':
                        os.write(master, b'\x1b[200~foo\x1b[200~bar\x1b[201~')
                        expected = 'foobar'
                    else:
                        gap = .05 if mode == 'escape-bytewise-50' else .02 if mode == 'escape-bytewise' else int(mode.split('-')[1]) / 1000
                        payload = b'\x1b[200~bar\x1b[201~'
                        if mode.startswith('escape-bytewise'):
                            write_parts([bytes([byte]) for byte in payload], gap)
                        else:
                            write_parts([payload[:1], payload[1:]], gap)
                        expected = 'bar'
                    wait_for(lambda: 'Tags: ' + expected in screen(output), 'fragmented/nested paste damaged prompt')
                    assert '[200~' not in screen(output), 'nested delimiter leaked into prompt'
                    os.write(master, b'\r')
                    state = root / 'state/state.toml'
                    wait_for(lambda: state.exists(), 'tags were not saved')
                    saved = tomllib.loads(state.read_text())
                    assert list(saved['tags'].values()) == [[expected]], saved
                    assert saved['pins'] == [] and saved['archived'] == [], saved
                until = time.monotonic() + .15
                while time.monotonic() < until: pump()
                os.write(master, b'q')
            elif mode == 'paste-handoff':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'n')
                wait_for(lambda: 'Agent:' in screen(output), 'agent picker missing')
                os.write(master, b'c')
                wait_for(lambda: (root / 'ready').exists(), 'child did not start')
                os.write(master, b'WORLD\n\x1b[200~foo')
                wait_for(lambda: (root / 'finished').exists() and 'launched ' in screen(output), 'unterminated paste broke child handoff')
                assert (root / 'agent-input').read_text() == 'WORLD'
                until = time.monotonic() + .4
                while time.monotonic() < until: pump()
                os.write(master, b'q')
            elif mode == 'typeahead':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'n')
                wait_for(lambda: 'Agent:' in screen(output), 'agent picker missing')
                os.write(master, b'cp\n')
                wait_for(lambda: (root / 'ready').exists(), 'child did not start')
                until = time.monotonic() + .1
                while time.monotonic() < until: pump()
                assert not (root / 'finished').exists(), 'discard policy passed type-ahead to the child'
                os.write(master, b'WORLD\n')
                wait_for(lambda: (root / 'finished').exists() and 'launched ' in screen(output), 'child did not return')
                assert (root / 'agent-input').read_text() == 'WORLD'
                until = time.monotonic() + .15
                while time.monotonic() < until: pump()
                state = root / 'state/state.toml'
                assert not state.exists() or tomllib.loads(state.read_text())['pins'] == [], 'type-ahead replayed as pin'
                os.write(master, b'q')
            elif mode in ('paste-long', 'paste-lock'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                if mode == 'paste-lock':
                    os.write(master, b't')
                    wait_for(lambda: 'Tags:' in screen(output), 'tag prompt missing')
                    state_dir = root / 'state'
                    state_dir.mkdir(exist_ok=True)
                    lock_fd = os.open(state_dir, os.O_RDONLY)
                    fcntl.flock(lock_fd, fcntl.LOCK_EX)
                    def release_lock():
                        time.sleep(.3)
                        fcntl.flock(lock_fd, fcntl.LOCK_UN)
                        os.close(lock_fd)
                    release = threading.Thread(target=release_lock)
                    release.start()
                    os.write(master, b'\x1b[200~foo\x1b[201~\rp\x1b[201~')
                    release.join(timeout=1)
                    wait_for(lambda: (state_dir / 'state.toml').exists(), 'tag save did not finish')
                else:
                    os.write(master, b'\x1b[200~foo\x1b[201~' + b'x' * 100 + b'p\x1b[201~')
                until = time.monotonic() + .4
                while time.monotonic() < until: pump()
                state = root / 'state/state.toml'
                assert proc.poll() is None, 'paste suffix quit the application'
                assert not state.exists() or tomllib.loads(state.read_text())['pins'] == [], 'busy UI let the paste suffix pin'
                if mode == 'paste-lock': assert list(tomllib.loads(state.read_text())['tags'].values()) == [['foo']]
                os.write(master, b'q')
            elif mode == 'paste-injection':

                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                os.write(master, b'\x1b[200~foo\x1b[201~pq\x1b[201~')
                until = time.monotonic() + .12
                while time.monotonic() < until: pump()
                assert proc.poll() is None, 'paste terminator injection quit the app'
                assert not (root / 'state/state.toml').exists(), 'paste terminator injection saved a pin'
                os.write(master, b'q')
            elif mode == 'paste':
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                assert b'\x1b[?2004h' in output, 'bracketed paste not enabled'
                os.write(master, b't')
                wait_for(lambda: 'Tags:' in screen(output), 'tag prompt missing')
                os.write(master, b'\x1b[200~foo,bar\np\nq\x1b[201~')
                wait_for(lambda: 'foo,barpq' in screen(output), 'paste did not stay in prompt')
                os.write(master, b'\r')
                state = root / 'state/state.toml'
                wait_for(lambda: state.exists(), 'tags were not saved')
                saved = tomllib.loads(state.read_text())
                assert list(saved['tags'].values()) == [['barpq', 'foo']], saved
                assert saved['pins'] == [] and saved['archived'] == [], saved
                assert proc.poll() is None, 'paste quit the application'
                # A deliberate quit follows the paste-burst quarantine.
                until = time.monotonic() + .06
                while time.monotonic() < until: pump()
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
            elif mode in ('resume', 'attention-resume', 'launch'):
                wait_for(lambda: '1 projects' in screen(output), 'initial scan did not arrive')
                if mode in ('resume', 'attention-resume'):
                    os.write(master, b'\t\r' if mode == 'attention-resume' else b'j\r')
                else:
                    os.write(master, b'n')
                    wait_for(lambda: 'Agent: c claude_code' in screen(output), 'no agent prompt')
                    print(screen(output))
                    os.write(master, b'c')
                wait_for(lambda: (root / 'flags').exists() and output.count(b'\x1b[?1049h') >= 2, 'agent did not return to alternate screen')
                flags = (root / 'flags').read_text()
                assert re.search(r'(?<!-)\bicanon\b', flags), flags
                assert re.search(r'(?<!-)\becho\b', flags), flags
                wait_for(lambda: ('resumed ' if mode in ('resume', 'attention-resume') else 'launched ') in screen(output), 'outcome missing after return')
                if mode == 'attention-resume':
                    assert (root / 'attention-args').read_text().splitlines() == ['--resume=waiting']
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
            stage = 'final terminal output'
            output += drain(master)
            if mode in ('termios', 'cli-termios'):
                flags = (root / 'exit-flags').read_text()
                for flag in ('icanon', 'echo', 'icrnl'):
                    assert re.search(r'(?<!-)\b' + flag + r'\b', flags), flags
            if direct: typescript.write_bytes(output)
            data = typescript.read_bytes()
            frame = data.index(b'Projects')
            assert b'\x1b[?1049l' in data[frame:], f'{mode}: alternate screen not restored'
            assert b'\x1b[?2004l' in data[frame:], f'{mode}: bracketed paste not disabled'
            assert b'\x1b[?25h' in data[frame:], f'{mode}: cursor not restored'
            if direct or sys.platform != 'darwin':
                expected = 128 + getattr(signal, 'SIG' + mode) if mode in ('TERM','INT','HUP') else (86 if mode == 'hook' else 0)
                assert proc.returncode == expected, (mode, proc.returncode, expected)
            print(f'{mode}: frame, exit, alternate-screen and cursor restoration passed')
        finally:
            if proc.poll() is None:
                if pidfile.exists():
                    try: os.kill(int(pidfile.read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass
                proc.kill()
                proc.wait(timeout=5)
            os.close(master)
            os.close(slave)

if __name__ == '__main__':
    BINARY = str(Path(sys.argv[1]).resolve())
    PROBE = str(Path(sys.argv[2]).resolve()) if len(sys.argv) > 2 else None
    for mode in ([os.environ['SBOARD_PTY_CASE']] if 'SBOARD_PTY_CASE' in os.environ else ['TERM'] if PROBE is None else ['TERM', 'INT', 'HUP', 'q', 'scan-panic', 'error', 'hook', 'dumb', 'stdin', 'idle', 'color-empty', 'color-off', 'unwatched', 'resume', 'attention-resume', 'launch', 'termios', 'child-int', 'cli-int', 'cli-open-int', 'message', 'stale-pane', 'socket-mismatch', 'vanished-pane', 'jump-failure', 'attach-failure', 'cli-termios', 'paste', 'paste-injection', 'typeahead', 'paste-long', 'paste-lock', 'escape-1', 'escape-20', 'escape-40', 'escape-bytewise', 'escape-bytewise-50', 'escape-alt', 'escape-alt-board', 'escape-alone', 'paste-timeout', 'paste-handoff', 'paste-nested', 'decoder-keys', 'escape-sequence-prefix', 'attention-fresh-pane', 'attention-collapsed-read', 'attention-read-in-child', 'tree-read-in-child', 'duplicate-open']):
        case(mode)
