"""Run isolated experiments in sequence. Build all binaries before timing."""
from pathlib import Path
import argparse
import datetime
import hashlib
import json
import os
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('mode', choices=['check', 'bench'])
parser.add_argument('--build', action='store_true')
parser.add_argument('--repeat', type=int, default=1)
args = parser.parse_args()
if args.repeat < 1:
    parser.error('--repeat must be positive')
sample = ROOT / '.tmp/dod-sample'
results = ROOT / '.tmp/dod-results'
results.mkdir(parents=True, exist_ok=True)
target = ROOT / '.tmp/dod-target'
environment = os.environ.copy()
environment['CARGO_TARGET_DIR'] = str(target)
groups = ['gates', 'gui', 'store', 'match']
if args.build:
    for group in groups:
        subprocess.run([sys.executable, str(ROOT / 'scripts/perf/prepare.py'), group], check=True)
        with (results / f'build-{group}.txt').open('w') as log:
            subprocess.run(['cargo', 'build', '--release', '--offline', '--manifest-path',
                            str(ROOT / f'.tmp/dod-{group}/Cargo.toml')], cwd=ROOT,
                           env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
executables = {
    'gates': ['dod-gates'],
    'gui': ['muzik-dod-gui-experiment', str(sample / 'tracks.json')],
    'store': ['muzik-dod-store', str(sample / 'tracks.json')],
    'match': ['muzik-dod-match-experiments', '--sample', str(sample)],
}
manifest = {
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
    'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'platform': platform.platform(), 'machine': platform.machine(),
    'mode': args.mode, 'repeat': args.repeat,
    'rustc': subprocess.check_output(['rustc', '-V'], env=environment, text=True).strip(),
    'profile': {'opt-level': 'z', 'lto': True, 'codegen-units': 1, 'strip': 'symbols'},
    'sample': {key: value for key, value in json.loads((sample / 'manifest.json').read_text()).items()
               if key != 'source_sha256'},
    'production_sources_sha256': {},
}
for source in sorted((ROOT / 'crates').glob('*/src/*.rs')) + sorted((ROOT / 'apps').glob('*/src/*.rs')):
    manifest['production_sources_sha256'][str(source.relative_to(ROOT))] = hashlib.sha256(source.read_bytes()).hexdigest()
(results / f'{args.mode}-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
failures = []
for repeat in range(args.repeat):
    for group in (groups if repeat % 2 == 0 else list(reversed(groups))):
        binary, *arguments = executables[group]
        command = [str(target / 'release' / binary), '--' + args.mode, *arguments]
        prefix = f'{args.mode}-{repeat + 1}-{group}'
        with (results / f'{prefix}.jsonl').open('w') as output, (results / f'{prefix}.stderr').open('w') as errors:
            result = subprocess.run(command, cwd=ROOT, env=environment, stdout=output, stderr=errors)
        if result.returncode:
            failures.append(group)
            print(f'{group}: execution failed; see {prefix}.stderr', flush=True)
        else:
            print(f'{group}: completed run {repeat + 1}', flush=True)
if failures:
    raise SystemExit('Experiment execution failed: ' + ', '.join(failures))
print(f'Raw results: {results}')
print('Read boundary records before accepting a candidate. A completed run can contain a rejected candidate.')
