from pathlib import Path
import shutil,re
root=Path('@ROOT@'); out=Path('@LAB@')
src=out/'src'; src.mkdir(exist_ok=True)
store=out/'store-candidate'; shutil.copytree(root/'crates/muzik-store',store,dirs_exist_ok=True)
p=store/'Cargo.toml';s=p.read_text().replace('name = "muzik-store"','name = "muzik-store-dod"');s=re.sub(r'path = "\.\./([^"]+)"',lambda m:f'path = "{root}/crates/{m[1]}"',s);p.write_text(s)
p=store/'src/watchlist/view.rs';s=p.read_text();s=s.replace('use std::collections::HashMap;','')
s=s.replace('pub actions: HashMap<ItemAction, Availability>','pub actions: ActionAvailability')
a=s.index('        let statuses: Vec<StageStatus>');b=s.index('\n    }\n}',a)
s=s[:a]+'''        if item.is_unavailable() { return Self::Unavailable; }
        let (running, waiting, failed, done) = item.statuses().fold(
            (false, false, false, true),
            |(r, w, f, d), status| (r || status == StageStatus::Running,
                w || status == StageStatus::Waiting, f || status == StageStatus::Failed,
                d && status.is_done()));
        if running { Self::Processing } else if waiting { Self::Waiting }
        else if failed { Self::Failed } else if done { Self::Processed }
        else { Self::Pending }'''+s[b:]
s+='''
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionAvailability { values: [Option<Availability>; 8] }
fn action_index(action: ItemAction) -> usize {
    match action {
        ItemAction::Run=>0, ItemAction::Retry=>1, ItemAction::DownloadAgain=>2,
        ItemAction::CheckQualityAgain=>3, ItemAction::ParseAgain=>4,
        ItemAction::SplitAgain=>5, ItemAction::OrganizeAgain=>6, ItemAction::RunAllAgain=>7,
    }
}
impl ActionAvailability {
    pub fn get(&self, action: &ItemAction) -> Option<&Availability> {
        self.values.get(action_index(*action)).and_then(Option::as_ref)
    }
}
impl FromIterator<(ItemAction,Availability)> for ActionAvailability {
    fn from_iter<I: IntoIterator<Item=(ItemAction,Availability)>>(iter:I)->Self {
        let mut result=Self { values: std::array::from_fn(|_|None) };
        for (action,availability) in iter {
            if let Some(slot)=result.values.get_mut(action_index(action)) { *slot=Some(availability); }
        }
        result
    }
}
impl Serialize for ActionAvailability {
    fn serialize<S: serde::Serializer>(&self, serializer:S)->Result<S::Ok,S::Error> {
        use serde::set::SerializeMap;
        let mut map=serializer.serialize_map(None)?;
        for action in ItemAction::ALL {
            if let Some(value)=self.get(action) { map.serialize_entry(action,value)?; }
        }
        map.end()
    }
}
impl<'de> Deserialize<'de> for ActionAvailability {
    fn deserialize<D:serde::Deserializer<'de>>(deserializer:D)->Result<Self,D::Error> {
        let map=std::collections::HashMap::<ItemAction,Availability>::deserialize(deserializer)?;
        Ok(map.into_iter().collect())
    }
}
''';p.write_text(s)
# Exact production snapshot helper, except names are unique in this executable.
s=(root/'crates/muzik-runner/src/queue.rs').read_text();a=s.index('fn snapshot(store:');b=s.index('\n#[must_use]',a)
s=s[a:b].replace('fn snapshot(', 'pub fn baseline_snapshot(')
(src/'snapshot.rs').write_text('use muzik_store::jobs::{Store,Kind,Status};\nuse serde_json::{Value,json};\nfn job_id(id:i64)->String {format!("queue-{id}")}\n'+s)
# Original ranking code, with the query tokens moved out of the per-candidate path.
s=(root/'crates/muzik-soulseek/src/ranking.rs').read_text();s=s[:s.index('\n#[cfg(test)]')]
a=s.index('    let query_tokens:');b=s.index('    if !query_tokens.is_empty()',a);q=s[a:b]
s=s[:a]+s[b:];s=s.replace('    let mut ranked = candidates','    '+q.strip()+'\n    let mut ranked = candidates',1).replace('score(&candidate, query, prefer)','score(&candidate, &query_tokens, prefer)').replace('fn score(candidate: &Candidate, query: &str, prefer:', 'fn score(candidate: &Candidate, query_tokens: &HashSet<String>, prefer:')
a=s.index('    let audio: Vec<_>');b=s.index('    let haystack =',a)
s=s[:a]+'''    let mut score=0.0_f64;
    let mut audio_count=0usize;
    let mut numbered_count=0usize;
    let mut first_parent:Option<&str>=None;
    let mut same_parent=true;
    for file in &candidate.files {
        if format(file).is_none() { continue; }
        audio_count+=1;
        score=score.max(quality(file,prefer));
        numbered_count+=usize::from(numbered(name(&file.name)));
        let path=parent(&file.name);
        if let Some(first)=first_parent { same_parent &= first==path; }
        else { first_parent=Some(path); }
    }
    if audio_count>0 {score+=f64::from(u32::try_from(audio_count.min(30)).unwrap_or(30));}
    if same_parent && first_parent.is_some_and(|p|!p.is_empty()) {score+=8.0;}
    if audio_count>0 && numbered_count >= (audio_count/2).max(1) {score+=10.0;}
'''+s[b:];(src/'ranking_candidate.rs').write_text(s)
# Candidate upgrade selector. Use source-extracted token, overlap and quality helpers.
s=(root/'crates/muzik-workflow/src/upgrade.rs').read_text();a=s.index('pub const DURATION_TOLERANCE');b=s.index('\n/// # Errors',a)
helpers=s[a:b];a=helpers.index('#[must_use]\npub fn safe_match');b=helpers.index('\nfn version_tokens',a);helpers=helpers[:a]+helpers[b:]
head='''use muzik_core::{PreferredAudio,audio::AudioFormat};
use muzik_soulseek::{ranking::RankedCandidate,types::{Candidate,FileEntry}};
use muzik_workflow::upgrade::FlaggedTrack;
use std::collections::HashSet;
'''
select='''
pub fn select_upgrade(track:&FlaggedTrack,ranked:&[RankedCandidate],prefer:PreferredAudio)->Option<(Candidate,f64)> {
    let current=quality_score(track.quality.format.audio_format(),track.quality.format.is_lossless(),true,
        track.quality.bitrate_kbps,track.quality.sample_rate,track.quality.bit_depth,prefer);
    let artist=tokens(&track.artist); let title=tokens(&track.title);
    let versions=version_tokens(&format!("{} {}",track.title,track.album));
    let best=ranked.iter().enumerate().flat_map(|(ci,c)|c.candidate.files.iter().enumerate().map(move |(fi,f)|(ci,fi,c,f)))
        .filter_map(|(ci,fi,ranked,file)| {
            if ranked.candidate.username.trim().is_empty() {return None;}
            let fmt=muzik_soulseek::ranking::format(file)?;
            let text=tokens(&file.name);
            let file_title=tokens(file.name.rsplit(['/', '\\\\']).next().unwrap_or(""));
            if !overlap(&artist,&text) || !overlap(&title,&file_title) {return None;}
            if version_tokens(&file.name).iter().any(|v|!versions.contains(v)) {return None;}
            if track.duration.is_some_and(|d|file.duration_seconds.is_none_or(|s|(f64::from(s)-d).abs()>DURATION_TOLERANCE)) {return None;}
            let q=quality_score(Some(fmt),fmt.is_lossless(),true,file.bitrate_kbps,file.sample_rate_hz,file.bit_depth,prefer);
            (q>current).then_some((ci,fi,q,ranked.score))
        }).max_by(|l,r|l.2.total_cmp(&r.2).then_with(||l.3.total_cmp(&r.3)));
    best.map(|(ci,fi,_,score)| {
        let c=&ranked[ci].candidate;
        (Candidate{username:c.username.clone(),slots:c.slots,speed:c.speed,files:vec![c.files[fi].clone()]},score)
    })
}
'''
(src/'upgrade_candidate.rs').write_text(head+helpers+select)
# Keep shared result types for exact comparisons with the production ranking path.
p=src/'ranking_candidate.rs';s=p.read_text();a=s.index('#[derive(Debug, Clone)]');b=s.index('\npub fn search_query',a);p.write_text(s[:a]+'pub use muzik_soulseek::ranking::RankedCandidate;\n'+s[b:])
p=src/'upgrade_candidate.rs';s=p.read_text().replace('(f64::from(s)-d).abs()>DURATION_TOLERANCE','!((f64::from(s)-d).abs()<=DURATION_TOLERANCE)');p.write_text(s)
