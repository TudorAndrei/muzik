#!/usr/bin/env python3
"""Make private experimental crate copies. Do not edit the repository or sample."""
from pathlib import Path
import shutil,re,sys
root=Path(sys.argv[1] if len(sys.argv)>1 else '@ROOT@').resolve()
out=Path(__file__).resolve().parent
members=[]
def copy(name,package):
 dest=out/name
 if dest.exists(): shutil.rmtree(dest)
 shutil.copytree(root/'crates'/package,dest,ignore=shutil.ignore_patterns('target','tests','benches'))
 text=(dest/'Cargo.toml').read_text()
 text=text.replace(f'name = "{package}"',f'name = "{name}"',1)
 text=re.sub(r'path = "\.\./([^"]+)"',lambda m:f'path = "{root}/crates/{m[1]}"',text)
 text=re.sub(r'\n\[\[bench\]\][\s\S]*','\n',text)
 (dest/'Cargo.toml').write_text(text)
 members.append(name)
 return dest
# 1. Use owned attributes rather than clone them.
p=copy('trial-library-move','muzik-library')/'src/lib.rs';s=p.read_text()
s=s.replace('let attributes = self.all_attributes(', 'let mut attributes = self.all_attributes(')
s=s.replace('attributes.get(&id).cloned().unwrap_or_default()', 'attributes.remove(&id).unwrap_or_default()')
p.write_text(s)
# 2. Restrict one attribute query to the selected album, consume its groups.
p=copy('trial-library-batch','muzik-library')/'src/lib.rs';s=p.read_text()
needle='''        let mut items = Vec::new();
        for row in rows {'''
replacement='''        let mut attr_statement = self.connection.prepare(
            "SELECT entity_id, key, value FROM item_attributes WHERE entity_id IN (SELECT id FROM items WHERE album_id = ?1)"
        )?;
        let attribute_rows = attr_statement.query_map([album_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Value>(2)?))
        })?;
        let mut attributes: BTreeMap<i64, Fields> = BTreeMap::new();
        for row in attribute_rows {
            let (id, key, value) = row?;
            attributes.entry(id).or_default().insert(key, value);
        }
        let mut items = Vec::new();
        for row in rows {'''
assert s.count(needle)==1;s=s.replace(needle,replacement)
start=s.index('    pub fn items_for_album(');end=s.index('\n    /// # Errors',start)
part=s[start:end];assert part.count('self.attributes(Entity::Item, id)?')==1
s=s[:start]+part.replace('self.attributes(Entity::Item, id)?','attributes.remove(&id).unwrap_or_default()')+s[end:]
p.write_text(s)
# 3. Generate track penalties with the exact production body, but keep singletons inline.
p=copy('trial-match-stack','muzik-match')/'src/distance.rs';s=p.read_text()
start=s.index('pub fn track_distance(');end=s.index('\nfn plurality',start)
body=s[start:end].replace('pub fn track_distance(', 'fn inline_track_distance(',1)
body=body.replace('Result<Distance, Error>','Result<InlineDistance, Error>',1).replace('Distance::default()','InlineDistance::default()',1)
extra='''
#[derive(Default)]
struct InlineDistance { penalties: [Option<(DistanceKey, f64)>; 7], len: usize }
impl InlineDistance {
 fn add(&mut self, key: DistanceKey, value: f64) -> Result<(), Error> {
  if !(0.0..=1.0).contains(&value) { return Err(Error::InvalidPenalty { key, value }); }
  // Production track_distance emits each key at most once.
  let slot = self.penalties.get_mut(self.len).ok_or(Error::InvalidConfig("inline capacity"))?;
  *slot=Some((key,value)); self.len += 1; Ok(())
 }
 fn add_string(&mut self,key:DistanceKey,left:Option<&str>,right:Option<&str>)->Result<(),Error>{self.add(key,string_dist(left,right))}
 fn add_equality(&mut self,key:DistanceKey,equal:bool)->Result<(),Error>{self.add(key,if equal {0.0}else{1.0})}
 fn add_ratio(&mut self,key:DistanceKey,numerator:f64,denominator:f64)->Result<(),Error>{
  let value=if denominator==0.0{0.0}else{numerator.min(denominator).max(0.0)/denominator};self.add(key,value)
 }
 fn score(&self,config:&MatchConfig)->Result<f64,Error>{
  let mut max=0.0;
  for &(key,_) in self.penalties.iter().flatten(){let weight=config.distance_weights.get(&key).ok_or(Error::MissingWeight(key))?;max+=count_to_f64(1)*weight;}
  if max==0.0{return Ok(0.0)}
  let mut total=0.0;
  for &(key,value) in self.penalties.iter().flatten(){let weight=config.distance_weights.get(&key).ok_or(Error::MissingWeight(key))?;total += [value].iter().sum::<f64>()*weight;}
  Ok(total/max)
 }
}
pub fn experimental_matrix(items:&[MatchItem],tracks:&[MatchTrack],config:&MatchConfig)->Result<Vec<f64>,Error>{
 let mut costs=Vec::with_capacity(items.len().saturating_mul(tracks.len()));
 for item in items { for track in tracks { costs.push(inline_track_distance(item,track,false,config)?.score(config)?); } }
 Ok(costs)
}
'''
p.write_text(s+'\n'+body+'\n'+extra)
lib=p.parent/'lib.rs';lib.write_text(lib.read_text()+'\npub use distance::experimental_matrix;\n')
ranking=p.parent/'ranking.rs';ranking_source=ranking.read_text()
old_matrix="""    let mut costs = Vec::with_capacity(items.len().saturating_mul(tracks.len()));
    for item in items {
        for track in tracks {
            costs.push(track_distance(item, track, false, config)?.score(config)?);
        }
    }"""
assert ranking_source.count(old_matrix)==1
ranking.write_text(ranking_source.replace(old_matrix,"    let costs = crate::distance::experimental_matrix(items, tracks, config)?;"))

# 4. Prepare the closed weight set once per cost matrix; retain the public maps.
p=copy('trial-match-weights','muzik-match')/'src/distance.rs';s=p.read_text()
keybody=s[s.index('pub enum DistanceKey {')+len('pub enum DistanceKey {'):s.index('\n}',s.index('pub enum DistanceKey {'))]
keys=re.findall(r'^    (\w+),',keybody,re.M);assert len(keys)==20
mapping='\n'.join(f'    DistanceKey::{key} => {i},' for i,key in enumerate(keys))
extra='''
struct DenseWeights([Option<f64>;20]);
fn weight_index(key:DistanceKey)->usize{match key {
'''+mapping+'''
}}
impl Distance {
 fn score_dense(&self,weights:&DenseWeights)->Result<f64,Error>{
  let mut max=0.0;
  for (key,values) in &self.penalties {
   let weight=weights.0[weight_index(*key)].ok_or(Error::MissingWeight(*key))?;
   max += count_to_f64(values.len())*weight;
  }
  if max==0.0{return Ok(0.0);}
  let mut total=0.0;
  for (key,values) in &self.penalties {
   let weight=weights.0[weight_index(*key)].ok_or(Error::MissingWeight(*key))?;
   total += values.iter().sum::<f64>()*weight;
  }
  Ok(total/max)
 }
}
pub fn experimental_matrix(items:&[MatchItem],tracks:&[MatchTrack],config:&MatchConfig)->Result<Vec<f64>,Error>{
 let mut weights=DenseWeights([None;20]);
 for (&key,&weight) in &config.distance_weights { weights.0[weight_index(key)]=Some(weight); }
 let mut costs=Vec::with_capacity(items.len().saturating_mul(tracks.len()));
 for item in items {for track in tracks {costs.push(track_distance(item,track,false,config)?.score_dense(&weights)?);}}
 Ok(costs)
}
'''
p.write_text(s+'\n'+extra)
lib=p.parent/'lib.rs';lib.write_text(lib.read_text()+'\npub use distance::experimental_matrix;\n')
ranking=p.parent/'ranking.rs';ranking_source=ranking.read_text()
assert ranking_source.count(old_matrix)==1
ranking.write_text(ranking_source.replace(old_matrix,"    let costs = crate::distance::experimental_matrix(items, tracks, config)?;"))
# 5. Keep MBID output production body; change only the source-index lookup.
p=copy('trial-import-index','muzik-import')/'src/mbid_fill.rs';s=p.read_text();start=s.index('pub fn mbid_fields(');end=s.index('\n/// Write only',start);part=s[start:end]
needle='''    let items = album
        .items'''
replacement='''    let mut source_tracks = vec![None; album.items.len()];
    for &(source, track) in &candidate.assignment.pairs {
        if let Some(slot) = source_tracks.get_mut(source) {
            if slot.is_none() { *slot = Some(track); }
        }
    }
    let items = album
        .items'''
assert part.count(needle)==1;part=part.replace(needle,replacement)
needle='''            let track = candidate
                .assignment
                .pairs
                .iter()
                .find(|(source, _)| *source == index)
                .and_then(|(_, track)| release.tracks.get(*track));'''
replacement='''            let track = source_tracks.get(index).copied().flatten()
                .and_then(|track| release.tracks.get(track));'''
assert part.count(needle)==1;part=part.replace(needle,replacement);s=s[:start]+part+s[end:];p.write_text(s)
# Original crates remain workspace dependencies through absolute paths.
manifest='''[workspace]
resolver="3"
members=['''+','.join('"'+x+'"' for x in members)+''']
[workspace.package]
version="2.14.1"
edition="2024"
rust-version="1.98"
[workspace.lints.rust]
unsafe_op_in_unsafe_fn="deny"
[package]
name="muzik-dod-match-experiments"
version="0.1.0"
edition="2024"
[dependencies]
serde={version="1",features=["derive"]}
serde_json={version="1",features=["float_roundtrip"]}
'''
for alias,package in [('base_match','muzik-match'),('base_library','muzik-library'),('base_import','muzik-import'),('muzik_core','muzik-core'),('muzik_metadata','muzik-metadata')]:
 manifest+=f'{alias}={{package="{package}",path="{root}/crates/{package}"}}\n'
for name in members:manifest+=f'{name.replace("-","_")}={{package="{name}",path="{name}"}}\n'
manifest+='''[profile.release]
opt-level="z"
lto=true
codegen-units=1
strip="symbols"
'''
(out/'Cargo.toml').write_text(manifest)
shutil.copyfile(root/'Cargo.lock',out/'Cargo.lock')
print('Private crates and manifest ready. No build was run.')

# Keep the independent public fixture suites available in each copied crate.
for trial,package,alias in [
    ('trial-library-move','muzik-library','trial_library_move'),
    ('trial-library-batch','muzik-library','trial_library_batch'),
    ('trial-match-stack','muzik-match','trial_match_stack'),
    ('trial-match-weights','muzik-match','trial_match_weights'),
]:
    shutil.copytree(root/'crates'/package/'tests',out/trial/'tests',dirs_exist_ok=True)
    for test in (out/trial/'tests').glob('*.rs'):
        text=test.read_text().replace(package.replace('-','_'),alias)
        test.write_text(text)
