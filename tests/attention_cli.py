"""Global attention JSON and grouped text have independent, fixed expected orders."""
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory() as scratch:
    root = Path(scratch)
    now = datetime.now(timezone.utc)
    projects = json.loads(Path('tests/fixtures/attention-projects.json').read_text())
    for project in projects:
        cwd = root / project['name']
        cwd.mkdir()
        store = root / 'claude/projects' / project['name']
        store.mkdir(parents=True)
        for session in project['sessions']:
            records = [json.loads(line) for line in Path(f"fixtures/claude_code/project/{session['status']}.jsonl").read_text().splitlines()]
            age = 7200 if session['status'] == 'idle' else 1000 - session['last_activity_at']
            for row in records:
                row.update(sessionId=session['id'], cwd=str(cwd), timestamp=(now - timedelta(seconds=age)).isoformat().replace('+00:00', 'Z'))
            (store / (session['id'] + '.jsonl')).write_text(''.join(json.dumps(row) + '\n' for row in records))
    (root / 'codex/sessions').mkdir(parents=True)
    env = dict(os.environ, CLAUDE_CONFIG_DIR=str(root / 'claude'), CODEX_HOME=str(root / 'codex'),
               SBOARD_STATE_DIR=str(root / 'state'), SBOARD_CONFIG_DIR=str(root / 'config'))
    command = [str(Path(sys.argv[1]).resolve()), 'list', '--status', 'attention']
    result = subprocess.run(command + ['--json'], env=env, capture_output=True, check=True)
    sessions = json.loads(result.stdout)
    assert [s['id'] for s in sessions] == ['claude_code:b-wait', 'claude_code:g-wait', 'claude_code:a-wait', 'claude_code:g-error', 'claude_code:b-error', 'claude_code:a-done'], sessions
    result = subprocess.run(command, env=env, capture_output=True, text=True, check=True)
    groups = [Path(line.split(' (')[0]).name for line in result.stdout.splitlines() if line and not line.startswith(' ')]
    assert groups == ['beta', 'gamma', 'alpha'], groups
    (root / 'state').mkdir()
    (root / 'state/state.toml').write_text(f'read = {{ "claude_code:a-done" = {int(now.timestamp()) + 10} }}\n')
    result = subprocess.run(command + ['--json'], env=env, capture_output=True, check=True)
    assert [s['id'] for s in json.loads(result.stdout)] == ['claude_code:b-wait', 'claude_code:g-wait', 'claude_code:a-wait', 'claude_code:g-error', 'claude_code:b-error']
