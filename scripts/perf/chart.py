"""Build the standalone numeric result view. No server is required."""
from pathlib import Path
import json

ROOT = Path(__file__).resolve().parents[2]
result = json.loads((ROOT / 'scripts/perf/results/data-oriented.json').read_text())
labels = {
 'attribute-ownership': 'Move owned attributes', 'batch-album-attributes': 'Batch album attributes',
 'gate_inline': 'Inline held gates', 'indexed-weights': 'Indexed matching weights',
 'job_projection': 'Queue field projection', 'library_indices': 'Library snapshot indices',
 'library_summaries': 'Album summaries', 'read_serial_array': 'Read serial array',
 'soulseek_prepared_query_one_pass': 'Prepared Soulseek query', 'source-index-array': 'Source index array',
 'stack-track-score': 'Stack penalties', 'summary_count_array': 'Summary count array',
 'upgrade_index_winner': 'Clone upgrade winner', 'watch_queue_only': 'Queue-only row updates',
 'watch_rows_indices': 'Watchlist row indices', 'watchlist_fixed_actions_summary': 'Fixed actions and stage fold',
}
representative = {
 'attribute-ownership': 'all-items', 'batch-album-attributes': 'largest',
 'gate_inline': 'depth3_rounds100', 'indexed-weights': 'largest', 'job_projection': 'jobs-1000',
 'library_indices': 'full', 'library_summaries': 'full', 'read_serial_array': 'full',
 'soulseek_prepared_query_one_pass': 'peers-1000', 'source-index-array': 'tagged/reversed',
 'stack-track-score': 'largest', 'summary_count_array': 'full', 'upgrade_index_winner': 'peers-1000',
 'watch_queue_only': 'full', 'watch_rows_indices': 'full', 'watchlist_fixed_actions_summary': 'items-1000',
}
rows = []
for value in result['results']:
    rows.append(dict(candidate=value['candidate'], label=labels[value['candidate']], workload=value['workload'],
        before=value['before_ns']['median']/1000, after=value['after_ns']['median']/1000,
        ratio=1/value['speedup'], low=min(value['paired_ratios']), high=max(value['paired_ratios']),
        before_allocs=value['before_allocs'], after_allocs=value['after_allocs'],
        samples=len(value['before_ns']['samples']), group=value['group'],
        representative=representative[value['candidate']]==value['workload'],
        rejected=value['candidate']=='job_projection'))
data = json.dumps(rows, separators=(',', ':'))
html = '''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Music sample experiments</title><style>
:root {color-scheme:light dark}
html,body{background:transparent;color:var(--foreground,CanvasText);font-family:var(--font-sans,system-ui,sans-serif);margin:0}p{line-height:1.45;margin:0 0 12px}label{font-size:14px}select{font:inherit;max-width:100%;color:var(--foreground,CanvasText);background:var(--secondary,transparent);border:1px solid var(--border,GrayText);border-radius:4px;padding:5px;margin:0 0 12px}svg{width:100%;display:block;font-family:var(--font-sans,system-ui,sans-serif)}.muted{color:var(--muted-foreground,GrayText);font-size:13px}.fail{color:var(--destructive,CanvasText)}table{border-collapse:collapse;width:100%;font-size:12px;font-variant-numeric:tabular-nums}th,td{text-align:right;border-bottom:1px solid var(--border,GrayText);padding:5px 3px}th:first-child,td:first-child{text-align:left}summary{cursor:pointer;margin:14px 0 8px}strong{font-weight:600} .scroll{overflow:auto}button:focus-visible,select:focus-visible,summary:focus-visible{outline:2px solid var(--primary,LinkText)}
</style></head><body>
<p><strong>Data-oriented design tests with Music metadata</strong><br>5,586 tracks · 2,678 albums · 16 experiments · 14 samples per path</p>
<p class="muted">This view shows data-processing tests. It does not measure the app event loop or live peer traffic. Album sizes and descriptions come from the real database. Peer, queue, and lifecycle states are generated.</p>
<label for="choice">Show </label><select id="choice"><option value="all">All experiments: selected workloads</option></select>
<div id="chart"></div>
<p class="muted">Dots show trial time divided by current time. Left of 1 means less time. Lines show the minimum and maximum paired ratios. They are observed ranges, not confidence intervals. Select an experiment to see every workload.</p>
<p class="fail">Queue projection fails two JSON error checks. Its timing does not make that version acceptable.</p>
<p class="muted">The overview uses 120 tracks for batch reads and 99 tracks with real IDs for source indices. Most sampled albums have one track. Their small cases must also be checked. Summary setup improves repeated views and makes one view slower.</p>
<details><summary>Exact time and allocation results</summary><div class="scroll"><table><thead><tr><th>Workload</th><th>Current µs</th><th>Trial µs</th><th>Rust allocations<br>current → trial</th></tr></thead><tbody id="values"></tbody></table></div></details>
<p class="muted">Two complete runs. Seven alternating samples per run. All builds finished before timing. Warm database reads on a shared arm64 Mac, Rust 1.98.1, release opt-level=z and LTO. Counts include Rust allocation and reallocation calls, not SQLite C allocation or peak memory. Gate timing includes allocator counter updates. No source change is applied by these experiments.</p>
<script>
const data=DATA;
const select=document.getElementById('choice');
for(const [id,label] of [...new Map(data.map(r=>[r.candidate,r.label]))].sort((a,b)=>a[1].localeCompare(b[1]))){const o=document.createElement('option');o.value=id;o.textContent=label;select.append(o)}
function fmt(v){return v>=1000?(v/1000).toFixed(2)+' ms':v.toFixed(v<10?2:1)+' µs'}
function escapeText(s){return s.replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function draw(){
 let rows=data.filter(r=>select.value==='all'?r.representative:r.candidate===select.value);
 if(select.value==='all'){rows.push(...data.filter(r=>(r.candidate==='source-index-array'&&r.workload==='small/reversed')||(r.candidate==='library_summaries'&&r.workload==='full_cold1')))}
 rows.sort((a,b)=>a.ratio-b.ratio);
 const max=Math.max(1.3,...rows.map(r=>r.high))*1.03;
 const x0=255,x1=574,top=34,step=31,w=700,h=top+rows.length*step+32;
 const x=v=>x0+(x1-x0)*v/max;
 let out=`<svg viewBox="0 0 ${w} ${h}" role="img" aria-label="Trial time relative to current time; values below one are lower"><text x="${x0}" y="14" fill="var(--muted-foreground,GrayText)" font-size="12">Trial / current time</text>`;
 for(const tick of [0,0.5,1,1.5,2].filter(v=>v<=max)){out+=`<line x1="${x(tick)}" x2="${x(tick)}" y1="24" y2="${h-22}" stroke="var(--border,GrayText)" stroke-width="${tick===1?1:0.35}" ${tick===1?'stroke-dasharray="3 3"':''}/><text x="${x(tick)}" y="${h-7}" fill="var(--muted-foreground,GrayText)" font-size="12" text-anchor="middle">${tick}</text>`}
 rows.forEach((r,i)=>{let y=top+i*step;const color=r.rejected?'var(--destructive,CanvasText)':r.candidate==='watch_queue_only'?'var(--primary,LinkText)':'var(--foreground,CanvasText)';out+=`<text x="0" y="${y}" fill="var(--foreground,CanvasText)" font-size="12">${escapeText(r.label)}</text><text x="0" y="${y+12}" fill="var(--muted-foreground,GrayText)" font-size="10">${escapeText(r.workload)}${r.rejected?' · REJECTED':''}</text><line x1="${x(r.low)}" x2="${x(r.high)}" y1="${y-3}" y2="${y-3}" stroke="${color}" stroke-width="1.2"/><circle cx="${x(r.ratio)}" cy="${y-3}" r="3" fill="${color}"/><text x="${x1+12}" y="${y}" fill="${color}" font-size="12">${r.ratio.toFixed(2)}× time</text>`});out+='</svg>';
 document.getElementById('chart').innerHTML=out;
 document.getElementById('values').innerHTML=rows.map(r=>`<tr><td>${escapeText(r.label)}<br><span class="muted">${escapeText(r.workload)}</span></td><td>${fmt(r.before)}</td><td>${fmt(r.after)}</td><td>${Math.round(r.before_allocs).toLocaleString()} → ${Math.round(r.after_allocs).toLocaleString()}</td></tr>`).join('');
}
select.addEventListener('change',draw);draw();
</script></body></html>'''.replace('DATA', data)
(ROOT / 'scripts/perf/results/data-oriented.html').write_text(html + '\n')
print(ROOT / 'scripts/perf/results/data-oriented.html')
