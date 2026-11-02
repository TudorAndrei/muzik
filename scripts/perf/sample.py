"""Copy a read-only beets database and export private benchmark metadata."""
from pathlib import Path
import argparse
import collections
import hashlib
import json
import os
import sqlite3

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('database', nargs='?', default='~/Music/.library.db')
args = parser.parse_args()
source = Path(args.database).expanduser().resolve()
output = ROOT / '.tmp/dod-sample'
output.mkdir(parents=True, exist_ok=True)
destination = output / 'library.db'
if source == destination.resolve():
    raise SystemExit('The source and copy must be different files.')
connection = sqlite3.connect(source.as_uri() + '?mode=ro', uri=True)
connection.row_factory = sqlite3.Row
if destination.exists():
    destination.unlink()
with sqlite3.connect(destination) as copy:
    connection.backup(copy)
destination.chmod(0o444)
columns = ['id', 'album_id', 'title', 'artist', 'album', 'albumartist',
           'length', 'format', 'track', 'disc', 'disctotal', 'year', 'genre',
           'label', 'path', 'added', 'mb_albumid', 'mb_trackid']
tracks = []
for row in connection.execute('SELECT ' + ','.join(columns) + ' FROM items ORDER BY album_id,id'):
    track = dict(row)
    if isinstance(track['path'], bytes):
        track['path'] = os.fsdecode(track['path']).rstrip('\x00')
    tracks.append(track)
(output / 'tracks.json').write_text(json.dumps(tracks, ensure_ascii=False))
match_items = []
for track in tracks:
    match_items.append(dict(
        title=track['title'] or '', artist=track['artist'] or '',
        album=track['album'] or '', album_artist=track['albumartist'] or '',
        length=track['length'] or 0.0, track=track['track'] or 0,
        disc=track['disc'] or 0, disc_total=track['disctotal'] or 0,
        year=track['year'] or 0, media=track['format'] or '',
        label=track['label'] or '', album_id=track['mb_albumid'] or '',
        track_id=track['mb_trackid'] or ''))
(output / 'match_items.json').write_text(json.dumps(match_items, ensure_ascii=False))
counts = collections.Counter(t['album_id'] for t in tracks if t['album_id'] is not None)
buckets = collections.Counter()
for count in counts.values():
    label = next(label for maximum, label in [(1, '1'), (4, '2-4'), (10, '5-10'),
                 (20, '11-20'), (50, '21-50'), (2**64, '51+')] if count <= maximum)
    buckets[label] += 1
manifest = dict(
    source='user-provided read-only Music database',
    source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
    tracks=len(tracks), albums_with_items=len(counts),
    album_tracks_min=min(counts.values(), default=0),
    album_tracks_max=max(counts.values(), default=0),
    album_size_buckets=dict(buckets),
    item_attributes=connection.execute('SELECT count(*) FROM item_attributes').fetchone()[0],
    album_attributes=connection.execute('SELECT count(*) FROM album_attributes').fetchone()[0],
    database_mode='read-only SQLite backup',
    generated_states='Peer, job, and GUI lifecycle states are generated from sampled metadata.')
(output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(f"Copied metadata for {len(tracks)} tracks and {len(counts)} albums to {output}.")
