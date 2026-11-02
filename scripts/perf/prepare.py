"""Materialize isolated experiments. All generated source stays under .tmp."""
from pathlib import Path
import argparse
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = Path(__file__).resolve().parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('group', choices=['gui', 'store', 'match', 'gates'])
args = parser.parse_args()
lab = ROOT / '.tmp' / ('dod-' + args.group)
lab.mkdir(parents=True, exist_ok=True)
if args.group == 'gates':
    subprocess.run([sys.executable, str(SCRIPTS / 'gates.py'), str(ROOT), str(lab)], check=True)
else:
    templates = SCRIPTS / args.group
    for source in templates.rglob('*'):
        if not source.is_file():
            continue
        destination = lab / source.relative_to(templates)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(source.read_text().replace('@ROOT@', str(ROOT)).replace('@LAB@', str(lab)))
    script = lab / ('prepare.py' if args.group == 'store' else 'setup.py')
    subprocess.run([sys.executable, str(script), str(ROOT)], check=True)
print(lab / 'Cargo.toml')
