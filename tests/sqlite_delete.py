"""Fifty cold scans per source during DELETE commits; scratch stores only.

Run: python3 tests/sqlite_delete.py target/release/sboard
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

BINARY = str(Path(sys.argv[1]).resolve())


def check(agent):
    with tempfile.TemporaryDirectory() as tmp:
        home = Path(tmp)
        directory = home / ('grok-home/sessions' if agent == 'grok' else 'store')
        directory.mkdir(parents=True)
        db = directory / ('session_search.sqlite' if agent == 'grok' else 'opencode.db')
        writer = sqlite3.connect(db, check_same_thread=False, timeout=3)
        fixture = 'session_search.sql' if agent == 'grok' else 'opencode.sql'
        writer.executescript((Path('fixtures') / agent / fixture).read_text())
        writer.execute('PRAGMA journal_mode=DELETE')
        writer.execute('PRAGMA busy_timeout=3000')
        table = 'session_docs' if agent == 'grok' else 'session'
        field = 'updated_at' if agent == 'grok' else 'time_updated'
        now = int(time.time()) * (1 if agent == 'grok' else 1000)
        writer.execute(f'UPDATE {table} SET {field}=?', (now,))
        writer.commit()
        before = sorted(p.name for p in directory.iterdir())
        stop, ready = threading.Event(), threading.Event()
        errors, commits = [], [0]

        def write():
            try:
                while not stop.is_set():
                    commits[0] += 1
                    writer.execute(f'UPDATE {table} SET title=?', (f'commit-{commits[0]}',))
                    writer.commit()
                    ready.set()
                    stop.wait(.01)
            except BaseException as error:
                errors.append(error)
                ready.set()

        thread = threading.Thread(target=write)
        children = []
        thread.start()
        try:
            assert ready.wait(3), 'writer did not commit'
            for i in range(50):
                env = dict(os.environ, HOME=tmp,
                           SBOARD_STATE_DIR=str(home / f'state-{i}'),
                           SBOARD_CONFIG_DIR=str(home / f'config-{i}'),
                           CLAUDE_CONFIG_DIR=str(home / 'claude'),
                           CODEX_HOME=str(home / 'codex'))
                env.pop('TMUX', None)
                if agent == 'grok':
                    env['GROK_HOME'] = str(home / 'grok-home')
                else:
                    env['OPENCODE_DB'] = str(db)
                children.append(subprocess.Popen(
                    [BINARY, 'list', '--agent', agent, '--json'], env=env,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
            deadline, polls = time.monotonic() + 30, 0
            while any(child.poll() is None for child in children):
                assert time.monotonic() < deadline, 'reader timeout'
                # DELETE's transient journal belongs to the writer; WAL/SHM never appear.
                names = {p.name for p in directory.iterdir()}
                assert names <= {db.name, db.name + '-journal'}, names
                polls += 1
                time.sleep(.001)
            for child in children:
                out, err = child.communicate(timeout=1)
                assert child.returncode == 0, (child.returncode, err)
                assert not err, err
                rows = [s for project in json.loads(out) for s in project['sessions']]
                assert len(rows) == (2 if agent == 'grok' else 6), (len(rows), out)
                assert all(s['title'].startswith('commit-') for s in rows)
        finally:
            stop.set()
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.communicate()
            thread.join(timeout=5)
            assert not thread.is_alive(), 'writer failed to stop'
            writer.close()
        assert not errors, errors
        assert sorted(p.name for p in directory.iterdir()) == before
        print(f'{agent}: 50 concurrent lists, {commits[0]} DELETE commits, {polls} directory polls; '
              'zero warnings/discards; no WAL/SHM or persistent journal; all child processes reaped')


for agent in ['grok', 'opencode']:
    check(agent)
