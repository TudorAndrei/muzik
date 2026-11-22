use std::{sync::Arc, time::Duration};

use muzik_runner::library_info::{AlbumRow, LibraryInfo, date, duration, playtime};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, ViewExt, component, view},
};

use crate::{
    activity::truncate,
    auth,
    state::{backend, hub},
};

const PAGE_SIZE: usize = 100;
const LIFETIME: Duration = Duration::from_secs(30);
const TEXT_LIMIT: usize = 120;
const FORMAT_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    Artist,
    Album,
    Year,
    Tracks,
    Length,
    Format,
    Added,
}

impl Sort {
    const ALL: [Self; 7] = [
        Self::Artist,
        Self::Album,
        Self::Year,
        Self::Tracks,
        Self::Length,
        Self::Format,
        Self::Added,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Year => "Year",
            Self::Tracks => "Tracks",
            Self::Length => "Length",
            Self::Format => "Format",
            Self::Added => "Added",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    pub column: Sort,
    pub direction: Direction,
}

fn word(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        _ => String::new(),
    }
}

#[must_use]
pub fn select(albums: &[Arc<AlbumRow>], needle: &str, order: Option<Order>) -> Vec<usize> {
    let needle = needle.trim().to_lowercase();
    let mut chosen: Vec<usize> = albums
        .iter()
        .enumerate()
        .filter(|(_, album)| needle.is_empty() || album.matches(&needle))
        .map(|(index, _)| index)
        .collect();
    let Some(Order { column, direction }) = order else {
        return chosen;
    };
    let album = |index: &usize| albums.get(*index).map(Arc::as_ref);
    chosen.sort_by(|left, right| {
        let (Some(left), Some(right)) = (album(left), album(right)) else {
            return std::cmp::Ordering::Equal;
        };
        match column {
            Sort::Artist => (
                left.artist.to_lowercase(),
                left.year,
                left.title.to_lowercase(),
            )
                .cmp(&(
                    right.artist.to_lowercase(),
                    right.year,
                    right.title.to_lowercase(),
                )),
            Sort::Album => left.title.to_lowercase().cmp(&right.title.to_lowercase()),
            Sort::Year => left.year.cmp(&right.year),
            Sort::Tracks => left.tracks.len().cmp(&right.tracks.len()),
            Sort::Length => left.length().total_cmp(&right.length()),
            Sort::Format => left.format().cmp(&right.format()),
            Sort::Added => left.added.total_cmp(&right.added),
        }
    });
    if direction == Direction::Desc {
        chosen.reverse();
    }
    chosen
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
struct Lib {
    #[serde(skip_serializing_if = "String::is_empty")]
    q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<Sort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dir: Option<Direction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    album: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AlbumLine {
    id: i64,
    artist: String,
    title: String,
    year: String,
    tracks: usize,
    length: String,
    format: String,
    added: String,
    selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackLine {
    number: String,
    title: String,
    artist: String,
    length: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Detail {
    title: String,
    facts: Vec<(&'static str, String)>,
    tracks: Vec<TrackLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Summary {
    lines: Vec<String>,
}

fn year_text(year: Option<i64>) -> String {
    year.map(|year| year.to_string()).unwrap_or_default()
}

fn summary(info: &LibraryInfo, shown: usize) -> Summary {
    let albums = if shown == info.albums.len() {
        format!("{} albums", info.albums.len())
    } else {
        format!("{shown} of {} albums", info.albums.len())
    };
    let mut facts = vec![
        albums,
        format!("{} tracks", info.tracks),
        format!("{} artists", info.artists),
        playtime(info.length),
    ];
    if info.singletons > 0 {
        facts.push(format!("{} singletons", info.singletons));
    }
    Summary {
        lines: vec![
            facts.join(" · "),
            info.formats
                .iter()
                .take(FORMAT_LIMIT)
                .map(|(format, count)| format!("{format} {count}"))
                .collect::<Vec<_>>()
                .join(" · "),
            info.directory.display().to_string(),
        ],
    }
}

fn detail(album: &AlbumRow) -> Detail {
    let multiple_discs = album.tracks.iter().any(|track| track.disc > 1);
    let facts = [
        ("Artist", Some(album.artist.clone())),
        ("Year", album.year.map(|year| year.to_string())),
        ("Genre", Some(album.genre.clone())),
        ("Label", Some(album.label.clone())),
        ("Format", Some(album.format())),
        ("Length", Some(duration(album.length()))),
        ("Added", Some(date(album.added))),
        ("Folder", Some(album.path.clone())),
    ]
    .into_iter()
    .filter_map(|(label, value)| Some((label, value.filter(|value| !value.is_empty())?)))
    .collect();
    Detail {
        title: truncate(&album.title, TEXT_LIMIT),
        facts,
        tracks: album
            .tracks
            .iter()
            .map(|track| TrackLine {
                number: match (multiple_discs, track.number) {
                    (_, None) => String::new(),
                    (true, Some(number)) => format!("{}-{number}", track.disc),
                    (false, Some(number)) => number.to_string(),
                },
                title: truncate(&track.title, TEXT_LIMIT),
                artist: if track.artist.is_empty() || track.artist == album.artist {
                    String::new()
                } else {
                    truncate(&track.artist, TEXT_LIMIT)
                },
                length: duration(track.length),
            })
            .collect(),
    }
}

fn album_lines(
    library: &LibraryInfo,
    chosen: &[usize],
    current: usize,
    selected: Option<i64>,
) -> Vec<AlbumLine> {
    chosen
        .iter()
        .skip(current.saturating_sub(1).saturating_mul(PAGE_SIZE))
        .take(PAGE_SIZE)
        .filter_map(|index| {
            let album = library.albums.get(*index)?;
            Some(AlbumLine {
                id: album.id,
                artist: truncate(&album.artist, TEXT_LIMIT),
                title: truncate(&album.title, TEXT_LIMIT),
                year: year_text(album.year),
                tracks: album.tracks.len(),
                length: duration(album.length()),
                format: album.format(),
                added: date(album.added),
                selected: selected == Some(album.id),
            })
        })
        .collect()
}

#[query_params(error = bad_request)]
struct LibraryQuery {
    #[serde(default)]
    q: String,
    sort: Option<Sort>,
    dir: Option<Direction>,
    page: Option<usize>,
    album: Option<i64>,
    #[serde(default)]
    reload: bool,
}

enum Read {
    Ready(Arc<LibraryInfo>),
    Failed(String),
}

async fn info(cx: &Cx) -> Result<Read> {
    if let Some(cached) = hub(cx).library(LIFETIME) {
        return Ok(Read::Ready(cached));
    }
    let read = backend(cx)
        .run(|service| {
            let config =
                Some(service.defaults().config).filter(|config| !config.as_os_str().is_empty());
            service.library(config.as_deref())
        })
        .await?;
    Ok(match read {
        Ok(info) => Read::Ready(hub(cx).store_library(info)),
        Err(error) => Read::Failed(error.to_string()),
    })
}

#[page("/library")]
pub async fn page(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    let query = query_params::<LibraryQuery>(cx)?;
    if query.reload {
        hub(cx).forget_library();
    }
    let outcome = info(cx).await?;
    let order = query.sort.map(|column| Order {
        column,
        direction: query.dir.unwrap_or(Direction::Asc),
    });
    let here = Lib {
        q: query.q.trim().to_owned(),
        sort: order.map(|order| order.column),
        dir: order
            .map(|order| order.direction)
            .filter(|direction| *direction == Direction::Desc),
        page: None,
        album: None,
    };
    let reload = href!(page).query(&ReloadQuery { reload: true }).resolve(cx);
    let library = match outcome {
        Read::Ready(library) => library,
        Read::Failed(message) => {
            return Ok(view! {
            <h1>"Library"</h1>
            <p class="notice error">(truncate(&message, 300))</p>
            <div class="card">
                <div class="empty">"Muzik cannot read the library database. Check the Beets config in Settings."</div>
            </div>
            <p><a class="button" href=(reload)>"Refresh"</a></p>
            }
            .boxed());
        }
    };
    let chosen = select(&library.albums, &here.q, order);
    let pages = chosen.len().div_ceil(PAGE_SIZE).max(1);
    let current = query.page.unwrap_or(1).clamp(1, pages);
    let rows = album_lines(&library, &chosen, current, query.album);
    let opened = query
        .album
        .and_then(|id| library.albums.iter().find(|album| album.id == id))
        .map(|album| detail(album));
    let empty = library.albums.is_empty() && library.singletons == 0;
    let facts = summary(&library, chosen.len());
    let here_page = Lib {
        page: Some(current),
        ..here.clone()
    };
    let search = href!(page).resolve(cx);
    Ok(view! {
        <h1>"Library"</h1>
        if empty {
            <div class="card">
                <div class="empty">
                    "The library is empty. No albums are in " (library.database.display().to_string()) ". Import music from the Activity page."
                </div>
            </div>
        } else {
            library_summary(summary: facts)
            <form class="launcher" method="get" action=(search)>
                <input
                    id="library-search"
                    type="text"
                    name="q"
                    value=(here.q.clone())
                    placeholder="Search artist, album, genre, label or year"
                    autocomplete="off"
                >
                <input type="hidden" name="sort" value=(here.sort.as_ref().map(word).unwrap_or_default())>
                <input type="hidden" name="dir" value=(here.dir.as_ref().map(word).unwrap_or_default())>
                <button class="primary" type="submit">"Search"</button>
                <a class="button" href=(reload)>"Refresh"</a>
            </form>
            <div class="lib">
                album_table(rows: rows, here: here_page, pages: pages, total: chosen.len())
                album_detail(detail: opened)
            </div>
        }
    }
    .boxed())
}

#[derive(Serialize)]
struct ReloadQuery {
    reload: bool,
}

#[component]
async fn library_summary(cx: &Cx, summary: Summary) -> Result<impl View> {
    Ok(view! {
        <div class="summary">
            for line in summary.lines {
                <div class="meta">(line)</div>
            }
        </div>
    })
}

#[component]
async fn album_table(
    cx: &Cx,
    rows: Vec<AlbumLine>,
    here: Lib,
    pages: usize,
    total: usize,
) -> Result<impl View> {
    let current = here.page.unwrap_or(1);
    let go = |change: &dyn Fn(&mut Lib)| {
        let mut next = here.clone();
        next.page = None;
        next.album = None;
        change(&mut next);
        href!(page).query(&next).resolve(cx)
    };
    let heads: Vec<(&'static str, String, &'static str)> = Sort::ALL
        .into_iter()
        .map(|sort| {
            let active = here.sort == Some(sort);
            let descending = active && here.dir == Some(Direction::Desc);
            let link = go(&|next: &mut Lib| {
                next.sort = Some(sort);
                next.dir = (active && !descending).then_some(Direction::Desc);
            });
            let mark = match (active, descending) {
                (false, _) => "",
                (true, false) => " ↑",
                (true, true) => " ↓",
            };
            (sort.label(), link, mark)
        })
        .collect();
    let lines: Vec<(AlbumLine, String)> = rows
        .into_iter()
        .map(|line| {
            let mut next = here.clone();
            next.album = Some(line.id);
            let link = href!(page).query(&next).resolve(cx);
            (line, link)
        })
        .collect();
    let previous = (current > 1).then(|| {
        let mut next = here.clone();
        next.page = Some(current.saturating_sub(1));
        href!(page).query(&next).resolve(cx)
    });
    let following = (current < pages).then(|| {
        let mut next = here.clone();
        next.page = Some(current.saturating_add(1));
        href!(page).query(&next).resolve(cx)
    });
    let none = lines.is_empty();
    Ok(view! {
        <section class="card">
            <div class="lrow lhead">
                for (label, link, mark) in heads {
                    <a href=(link)>(label) (mark)</a>
                }
            </div>
            <div class="scroll libtable">
                if none {
                    <div class="empty">"No albums match."</div>
                }
                for (entry, target) in lines {
                    <div class=(if entry.selected { "lrow current" } else { "lrow" }) id=(format!("album-{}", entry.id))>
                        <span class="cell">(entry.artist.clone())</span>
                        <a class="cell title" href=(target)>(entry.title.clone())</a>
                        <span class="cell num">(entry.year.clone())</span>
                        <span class="cell num">(entry.tracks)</span>
                        <span class="cell num">(entry.length.clone())</span>
                        <span class="cell">(entry.format.clone())</span>
                        <span class="cell">(entry.added.clone())</span>
                    </div>
                }
            </div>
            <div class="pager">
                <span class="meta">(total) " albums · page " (current) " of " (pages)</span>
                if let Some(link) = previous {
                    <a class="button small" href=(link)>"Previous"</a>
                }
                if let Some(link) = following {
                    <a class="button small" href=(link)>"Next"</a>
                }
            </div>
        </section>
    })
}

#[component]
async fn album_detail(cx: &Cx, detail: Option<Detail>) -> Result<impl View> {
    Ok(view! {
        <section class="card">
            if let Some(detail) = detail {
                <h2>(detail.title)</h2>
                <div class="scroll libdetail">
                    <dl class="facts">
                        for (label, value) in detail.facts {
                            <dt>(label)</dt>
                            <dd>(value)</dd>
                        }
                    </dl>
                    <div class="trow thead">
                        <span>"#"</span>
                        <span>"Title"</span>
                        <span class="num">"Length"</span>
                    </div>
                    for track in detail.tracks {
                        <div class="trow">
                            <span class="meta">(track.number)</span>
                            <span class="tcell">
                                <span class="title">(track.title)</span>
                                <span class="meta">(track.artist)</span>
                            </span>
                            <span class="num meta">(track.length)</span>
                        </div>
                    }
                </div>
            } else {
                <h2>"Album"</h2>
                <div class="scroll libdetail">
                    <div class="empty">"Select an album to see its tracks."</div>
                </div>
            }
        </section>
    })
}

#[cfg(test)]
mod tests {
    use super::{Direction, Order, Sort, select};
    use muzik_runner::library_info::{AlbumRow, Track};
    use std::sync::Arc;

    fn album(artist: &str, title: &str, year: i64, added: f64) -> AlbumRow {
        AlbumRow {
            artist: artist.into(),
            title: title.into(),
            year: Some(year),
            added,
            ..AlbumRow::default()
        }
    }

    #[test]
    fn the_filter_and_sort_choose_the_shown_albums() {
        let mut older = album("Tommy Guerrero", "Road to Knowhere", 2011, 1.);
        older.tracks = vec![Track::default(); 3];
        let albums: Vec<Arc<AlbumRow>> = [
            older,
            album("Phoniks", "Down To Earth", 2015, 3.),
            album("Awon & Phoniks", "The Actual Proof", 2016, 2.),
        ]
        .map(Arc::new)
        .to_vec();
        assert_eq!(select(&albums, "", None), [0, 1, 2]);
        assert_eq!(select(&albums, " PHONIKS ", None), [1, 2]);
        assert_eq!(select(&albums, "2011", None), [0]);
        let by = |column, direction| Some(Order { column, direction });
        assert_eq!(
            select(&albums, "", by(Sort::Artist, Direction::Asc)),
            [2, 1, 0]
        );
        assert_eq!(
            select(&albums, "", by(Sort::Added, Direction::Desc)),
            [1, 2, 0]
        );
        assert_eq!(
            select(&albums, "", by(Sort::Tracks, Direction::Desc)).first(),
            Some(&0)
        );
    }
}
