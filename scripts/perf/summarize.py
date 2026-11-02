"""Normalize the timing and allocation records from run.py."""
from pathlib import Path
import collections
import json
import statistics

ROOT = Path(__file__).resolve().parents[2]
results = ROOT / '.tmp/dod-results'
output = ROOT / 'scripts/perf/results'
output.mkdir(parents=True, exist_ok=True)
values = collections.defaultdict(lambda: {'before': [], 'after': [], 'before_allocs': [], 'after_allocs': [], 'before_bytes': [], 'after_bytes': [], 'pairs': collections.defaultdict(dict)})
controls = []
for path in sorted(results.glob('bench-*.jsonl')):
    run = path.stem.split('-')[1]
    group = path.stem.split('-')[2]
    for line in path.read_text().splitlines():
        row = json.loads(line)
        if row.get('mode') == 'boundary':
            controls.append(row)
        if 'candidate' not in row:
            continue
        workload = row.get('workload', row.get('size'))
        if workload is None:
            continue
        entry = values[(row['candidate'], workload)]
        entry['group'] = group
        entry['records'] = row.get('records')
        if 'before_ns' in row:
            for side in ['before', 'after']:
                for index, ns in enumerate(row[side + '_ns']):
                    entry[side].append(ns)
                    entry['pairs'][(run, index)][side] = ns
                entry[side + '_allocs'].append(row[side + '_allocs'])
        elif 'baseline_ns' in row:
            for side, key in [('before', 'baseline_ns'), ('after', 'candidate_ns')]:
                entry[side].append(row[key])
                entry['pairs'][(run, row['sample'])][side] = row[key]
                entry[side + '_allocs'].append(row[side + '_allocs'])
                entry[side + '_bytes'].append(row[side + '_alloc_bytes'])
        elif 'elapsed_ns' in row:
            side = row['side']
            entry[side].append(row['elapsed_ns'] / row['iterations'])
            entry['pairs'][(run, row['sample'])][side] = row['elapsed_ns'] / row['iterations']
            entry[side + '_allocs'].append((row['allocations_total'] + row['reallocations_total']) / row['iterations'])
            entry[side + '_bytes'].append(row['requested_bytes_total'] / row['iterations'])
        elif row.get('mode') == 'bench':
            side = row['side']
            entry[side].append(row['sample_ns'])
            entry['pairs'][(run, row['sample'])][side] = row['sample_ns']
            entry[side + '_allocs'].append(row['sample_allocs'])
            entry[side + '_bytes'].append(row['sample_requested_bytes'])

def stats(samples):
    return {'median': statistics.median(samples), 'min': min(samples), 'max': max(samples), 'samples': samples}

summary = []
for (candidate, workload), entry in sorted(values.items()):
    if not entry['before'] or not entry['after']:
        continue
    before, after = stats(entry['before']), stats(entry['after'])
    item = dict(candidate=candidate, workload=workload, group=entry['group'], records=entry['records'], before_ns=before, after_ns=after, speedup=before['median'] / after['median'], paired_ratios=[pair['after'] / pair['before'] for pair in entry['pairs'].values() if 'after' in pair and 'before' in pair])
    for metric in ['allocs', 'bytes']:
        for side in ['before', 'after']:
            sample = entry[side + '_' + metric]
            if sample:
                item[side + '_' + metric] = statistics.median(sample)
    summary.append(item)
record = {'method': 'Two complete sequential runs; seven alternating samples per path per run. All builds finished before timing. Allocation counts include reallocations and come from separate untimed passes.',
          'limits': 'Shared Mac; warm local database reads; Rust allocation calls only, not SQLite C allocations or peak memory. Generated queue, peer, and GUI states use sampled Music descriptions. GUI kernels exclude window rendering. Results do not establish application latency.',
          'metadata': json.loads((results / 'bench-manifest.json').read_text()), 'boundary_controls': controls, 'results': summary}
# Only aggregate metadata and numeric results leave the private .tmp directory.
record['metadata'].pop('production_sources_sha256', None)
(output / 'data-oriented.json').write_text(json.dumps(record, indent=2) + '\n')
for row in summary:
    print(f"{row['candidate']:36} {row['workload']:32} {row['speedup']:6.2f}x  allocations {row.get('before_allocs','?')} -> {row.get('after_allocs','?')}")
print(f'Saved {len(summary)} workloads across {len({r["candidate"] for r in summary})} experiments.')
