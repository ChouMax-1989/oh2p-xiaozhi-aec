"""Check public export without printing matched secret values."""
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[1]
files = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).decode().split('\0')
rules = {
    'private-address': rb'(?<![0-9])192\.168\.\d{1,3}\.\d{1,3}(?![0-9])',
    'personal-path': rb'(?:[CD]:[/\\](?:Users[/\\]admin|AI[/\\]))',
    'private-key': rb'-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----',
    'github-token': rb'(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})',
    'openai-key': rb'sk-[A-Za-z0-9_-]{30,}',
}
failures = []
for name in filter(None, files):
    if name == 'tools/check_public.py':
        continue
    data = (root / name).read_bytes()
    for label, pattern in rules.items():
        if re.search(pattern, data):
            failures.append(f'{name}: {label}')
if failures:
    raise SystemExit('\n'.join(failures))
print(f'Checked {len(list(filter(None, files)))} tracked files: no configured privacy pattern matched.')
