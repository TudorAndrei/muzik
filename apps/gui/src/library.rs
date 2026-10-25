use super::*;
use crate::watchlist_view::empty_state;
use chrono::{DateTime, Local};
use gpui_kit::component::sheet::Sheet;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::table::{
    Column, ColumnSort, DataTable, Table, TableBody, TableCell, TableDelegate, TableHead,
    TableHeader, TableRow,
};
use muzik_library::{Album, Item, Library, SqlValue};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub disc: i64,
    pub number: i64,
    pub title: String,
    pub artist: String,
    pub length: f64,
    pub format: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumRow {
    pub artist: String,
    pub title: String,
    pub year: i64,
    pub genre: String,
    pub label: String,
    pub added: f64,
    pub path: String,
    pub tracks: Vec<Track>,
}

impl AlbumRow {
    fn length(&self) -> f64 {
        self.tracks.iter().map(|track| track.length).sum()
    }

    fn format(&self) -> String {
        self.tracks
            .iter()
            .map(|track| track.format.as_str())
            .filter(|format| !format.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn matches(&self, needle: &str) -> bool {
        [&self.artist, &self.title, &self.genre, &self.label]
            .iter()
            .any(|text| text.to_lowercase().contains(needle))
            || (self.year > 0 && self.year.to_string() == needle)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryInfo {
    pub database: PathBuf,
    pub directory: PathBuf,
    pub albums: Vec<Arc<AlbumRow>>,
    pub singletons: usize,
    pub tracks: usize,
    pub artists: usize,
    pub length: f64,
    pub formats: Vec<(String, usize)>,
}

fn text(value: Option<&SqlValue>) -> String {
    value
        .and_then(muzik_library::scalar_text)
        .unwrap_or_default()
}

fn integer(value: Option<&SqlValue>) -> i64 {
    match value {
        Some(SqlValue::Integer(number)) => *number,
        Some(SqlValue::Text(text)) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn real(value: Option<&SqlValue>) -> f64 {
    match value {
        Some(SqlValue::Real(number)) => *number,
        Some(SqlValue::Integer(number)) => i32::try_from(*number).map_or(0., f64::from),
        Some(SqlValue::Text(text)) => text.trim().parse().unwrap_or(0.),
        _ => 0.,
    }
}

fn track(item: &Item) -> Track {
    Track {
        disc: integer(item.field("disc")),
        number: integer(item.field("track")),
        title: text(item.field("title")),
        artist: text(item.field("artist")),
        length: real(item.field("length")),
        format: text(item.field("format")).to_uppercase(),
        path: item.field("path").and_then(muzik_library::path_from_sql),
    }
}

fn album_row(album: &Album, tracks: Vec<Track>) -> AlbumRow {
    AlbumRow {
        artist: text(album.field("albumartist")),
        title: text(album.field("album")),
        year: integer(album.field("year")),
        genre: text(album.field("genre")),
        label: text(album.field("label")),
        added: real(album.field("added")),
        path: tracks
            .iter()
            .find_map(|track| track.path.as_deref()?.parent())
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        tracks,
    }
}

pub fn read(database: &Path, directory: &Path) -> anyhow::Result<LibraryInfo> {
    let library = Library::open_read_only(database)?;
    let items = library.items()?;
    let mut by_album: BTreeMap<i64, Vec<Track>> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut artists = BTreeSet::new();
    let mut singletons = 0_usize;
    let mut length = 0.;
    for item in &items {
        let track = track(item);
        length += track.length;
        if !track.format.is_empty() {
            let count = formats.entry(track.format.clone()).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(id) = item.album_id() {
            by_album.entry(id).or_default().push(track);
        } else {
            if !track.artist.is_empty() {
                artists.insert(track.artist.to_lowercase());
            }
            singletons = singletons.saturating_add(1);
        }
    }
    let mut albums = Vec::new();
    for album in library.albums()? {
        let mut tracks = by_album.remove(&album.id).unwrap_or_default();
        tracks.sort_by_key(|track| (track.disc, track.number));
        let row = album_row(&album, tracks);
        if !row.artist.is_empty() {
            artists.insert(row.artist.to_lowercase());
        }
        albums.push(Arc::new(row));
    }
    albums.sort_by(|left, right| right.added.total_cmp(&left.added));
    let mut formats: Vec<_> = formats.into_iter().collect();
    formats.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    Ok(LibraryInfo {
        database: database.to_path_buf(),
        directory: directory.to_path_buf(),
        albums,
        singletons,
        tracks: items.len(),
        artists: artists.len(),
        length,
        formats,
    })
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "std has no TryFrom<f64> for u64; the value is finite and positive"
)]
fn seconds(length: f64) -> u64 {
    if length.is_finite() && length > 0. {
        length.round() as u64
    } else {
        0
    }
}

pub fn duration(length: f64) -> String {
    let total = seconds(length);
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

pub fn playtime(length: f64) -> String {
    let total = seconds(length);
    let (days, hours) = (total / 86_400, total / 3600 % 24);
    if days > 0 {
        format!("{days} d {hours} h")
    } else {
        format!("{hours} h {} min", total / 60 % 60)
    }
}

fn date(added: f64) -> String {
    i64::try_from(seconds(added))
        .ok()
        .filter(|stamp| *stamp > 0)
        .and_then(|stamp| DateTime::from_timestamp(stamp, 0))
        .map(|at| at.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryColumn {
    Artist,
    Album,
    Year,
    Tracks,
    Length,
    Format,
    Added,
}

impl LibraryColumn {
    const ALL: [Self; 7] = [
        Self::Artist,
        Self::Album,
        Self::Year,
        Self::Tracks,
        Self::Length,
        Self::Format,
        Self::Added,
    ];

    fn at(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Year => "year",
            Self::Tracks => "tracks",
            Self::Length => "length",
            Self::Format => "format",
            Self::Added => "added",
        }
    }

    const fn name(self) -> &'static str {
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

    const fn width(self) -> f32 {
        match self {
            Self::Artist => 220.,
            Self::Album => 280.,
            Self::Year | Self::Tracks => 64.,
            Self::Length => 80.,
            Self::Format => 96.,
            Self::Added => 104.,
        }
    }

    const fn numeric(self) -> bool {
        match self {
            Self::Year | Self::Tracks | Self::Length => true,
            Self::Artist | Self::Album | Self::Format | Self::Added => false,
        }
    }

    const fn muted(self) -> bool {
        match self {
            Self::Artist | Self::Album => false,
            Self::Year | Self::Tracks | Self::Length | Self::Format | Self::Added => true,
        }
    }
}

pub struct LibraryTable {
    albums: Vec<Arc<AlbumRow>>,
    rows: Vec<Arc<AlbumRow>>,
    filter: String,
    sort: Option<(LibraryColumn, ColumnSort)>,
    loading: bool,
}

impl LibraryTable {
    pub(crate) const fn new() -> Self {
        Self {
            albums: Vec::new(),
            rows: Vec::new(),
            filter: String::new(),
            sort: None,
            loading: false,
        }
    }

    pub(crate) fn set_albums(&mut self, albums: Vec<Arc<AlbumRow>>) {
        self.albums = albums;
        self.loading = false;
        self.apply();
    }

    pub(crate) const fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    pub(crate) fn set_filter(&mut self, filter: &str) {
        self.filter = filter.trim().to_lowercase();
        self.apply();
    }

    pub(crate) fn album(&self, row_ix: usize) -> Option<Arc<AlbumRow>> {
        self.rows.get(row_ix).cloned()
    }

    pub(crate) const fn shown(&self) -> usize {
        self.rows.len()
    }

    fn apply(&mut self) {
        let filter = self.filter.as_str();
        self.rows = self
            .albums
            .iter()
            .filter(|album| filter.is_empty() || album.matches(filter))
            .cloned()
            .collect();
        let Some((column, order)) = self.sort.filter(|(_, order)| *order != ColumnSort::Default)
        else {
            return;
        };
        match column {
            LibraryColumn::Artist => self.rows.sort_by_cached_key(|album| {
                (
                    album.artist.to_lowercase(),
                    album.year,
                    album.title.to_lowercase(),
                )
            }),
            LibraryColumn::Album => self
                .rows
                .sort_by_cached_key(|album| album.title.to_lowercase()),
            LibraryColumn::Year => self.rows.sort_by_key(|album| album.year),
            LibraryColumn::Tracks => self.rows.sort_by_key(|album| album.tracks.len()),
            LibraryColumn::Length => self
                .rows
                .sort_by(|left, right| left.length().total_cmp(&right.length())),
            LibraryColumn::Format => self.rows.sort_by_cached_key(|album| album.format()),
            LibraryColumn::Added => self
                .rows
                .sort_by(|left, right| left.added.total_cmp(&right.added)),
        }
        if order == ColumnSort::Descending {
            self.rows.reverse();
        }
    }

    fn text(&self, row_ix: usize, column: Option<LibraryColumn>) -> String {
        let (Some(album), Some(column)) = (self.rows.get(row_ix), column) else {
            return String::new();
        };
        match column {
            LibraryColumn::Artist => album.artist.clone(),
            LibraryColumn::Album => album.title.clone(),
            LibraryColumn::Year if album.year > 0 => album.year.to_string(),
            LibraryColumn::Year => String::new(),
            LibraryColumn::Tracks => album.tracks.len().to_string(),
            LibraryColumn::Length => duration(album.length()),
            LibraryColumn::Format => album.format(),
            LibraryColumn::Added => date(album.added),
        }
    }
}

impl TableDelegate for LibraryTable {
    fn columns_count(&self, _: &App) -> usize {
        LibraryColumn::ALL.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let Some(column) = LibraryColumn::at(col_ix) else {
            return Column::new("", "");
        };
        let header = Column::new(column.key(), column.name())
            .width(px(column.width()))
            .sortable();
        if column.numeric() {
            header.text_right()
        } else {
            header
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut gpui_kit::Context<gpui_kit::component::table::TableState<Self>>,
    ) {
        self.sort = LibraryColumn::at(col_ix).map(|column| (column, sort));
        self.apply();
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut gpui_kit::Context<gpui_kit::component::table::TableState<Self>>,
    ) -> impl IntoElement {
        let column = LibraryColumn::at(col_ix);
        let cell = div().min_w_0().truncate().child(self.text(row_ix, column));
        if column.is_none_or(LibraryColumn::muted) {
            cell.text_color(cx.theme().muted_foreground)
        } else {
            cell
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.text(row_ix, LibraryColumn::at(col_ix))
    }
}

impl Muzik {
    pub(crate) fn library(&self, cx: &Context<Self>) -> AnyElement {
        let loading = self.reading(Read::Library);
        let refresh = Button::new("library-refresh")
            .ghost()
            .icon(IconName::RefreshCw)
            .label("Refresh")
            .disabled(loading)
            .on_click(cx.listener(|view, _, _, cx| {
                view.load_library(cx);
                cx.notify();
            }));
        let mut page = pages::page_frame()
            .flex_1()
            .min_h_0()
            .child(pages::page_header("Library", refresh));
        page = match &self.library {
            Some(info) if !info.albums.is_empty() || info.singletons > 0 => page
                .child(self.library_summary(info, cx))
                .child(Input::new(&self.library_search).cleanable(true))
                .child(
                    pages::table_frame()
                        .on_action(cx.listener(|view, _: &OpenSelected, window, cx| {
                            let table = view.library_table.read(cx);
                            let album = table
                                .selected_row()
                                .and_then(|row| table.delegate().album(row));
                            if let Some(album) = album {
                                Self::open_album_sheet(album, window, cx);
                            }
                        }))
                        .child(DataTable::new(&self.library_table).stripe(true)),
                ),
            Some(info) => page.child(empty_state(
                "The library is empty",
                format!(
                    "No albums are in {}. Import music from the Workflow page.",
                    info.database.display()
                ),
                false,
            )),
            None if loading => page
                .child(Self::library_summary_skeleton(cx))
                .child(
                    Input::new(&self.library_search)
                        .cleanable(true)
                        .disabled(true),
                )
                .child(
                    pages::table_frame().child(DataTable::new(&self.library_table).stripe(true)),
                ),
            None => page.child(empty_state(
                "No library",
                "Muzik cannot read the library database. Check the Beets config in Settings.",
                false,
            )),
        };
        div()
            .flex()
            .justify_center()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(page)
            .into_any_element()
    }

    fn library_summary_skeleton(cx: &App) -> Div {
        let line = |width: f32| {
            style::meta("\u{a0}", cx).relative().child(
                Skeleton::new()
                    .absolute()
                    .top(px(3.))
                    .bottom(px(3.))
                    .left_0()
                    .w(px(width))
                    .rounded_sm(),
            )
        };
        div()
            .v_flex()
            .gap_1()
            .child(line(360.))
            .child(line(220.))
            .child(line(280.))
    }

    fn library_summary(&self, info: &LibraryInfo, cx: &Context<Self>) -> Div {
        let shown = self.library_table.read(cx).delegate().shown();
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
        let formats = info
            .formats
            .iter()
            .take(4)
            .map(|(format, count)| format!("{format} {count}"))
            .collect::<Vec<_>>()
            .join(" · ");
        div()
            .v_flex()
            .gap_1()
            .child(style::meta(facts.join(" · "), cx))
            .child(style::meta(formats, cx))
            .child(
                style::mono(info.directory.display().to_string(), cx)
                    .text_color(cx.theme().muted_foreground)
                    .truncate(),
            )
    }

    pub(crate) fn apply_library(&mut self, info: LibraryInfo, cx: &mut Context<Self>) {
        let albums = info.albums.clone();
        self.library = Some(info);
        self.library_table.update(cx, |state, cx| {
            state.delegate_mut().set_albums(albums);
            cx.notify();
        });
    }

    pub(crate) fn filter_library(&self, cx: &mut Context<Self>) {
        let filter = self.library_search.read(cx).value().to_string();
        self.library_table.update(cx, |state, cx| {
            state.clear_selection(cx);
            state.delegate_mut().set_filter(&filter);
            cx.notify();
        });
    }

    pub(crate) fn open_album_sheet(
        album: Arc<AlbumRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.open_sheet(cx, move |sheet, _, cx| album_sheet(sheet, &album, cx));
    }
}

fn album_sheet(sheet: Sheet, album: &AlbumRow, cx: &App) -> Sheet {
    let mut facts = DescriptionList::horizontal()
        .columns(1)
        .label_width(px(90.))
        .item("Artist", album.artist.clone(), 1);
    for (label, value) in [
        ("Year", (album.year > 0).then(|| album.year.to_string())),
        ("Genre", Some(album.genre.clone())),
        ("Label", Some(album.label.clone())),
        ("Format", Some(album.format())),
        ("Length", Some(duration(album.length()))),
        ("Added", Some(date(album.added))),
        ("Folder", Some(album.path.clone())),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            facts = facts.item(label, value, 1);
        }
    }
    let discs = album.tracks.iter().any(|track| track.disc > 1);
    let mut body = TableBody::new();
    for track in &album.tracks {
        let number = match (discs, track.number) {
            (_, 0) => String::new(),
            (true, number) => format!("{}-{number}", track.disc),
            (false, number) => number.to_string(),
        };
        let mut title = div()
            .v_flex()
            .min_w_0()
            .child(div().truncate().child(track.title.clone()));
        if !track.artist.is_empty() && track.artist != album.artist {
            title = title.child(style::meta(track.artist.clone(), cx).truncate());
        }
        body = body.child(
            TableRow::new()
                .child(TableCell::new().w(px(44.)).child(style::mono(number, cx)))
                .child(TableCell::new().min_w_0().flex_1().child(title))
                .child(
                    TableCell::new()
                        .w(px(64.))
                        .text_right()
                        .child(style::mono(duration(track.length), cx)),
                ),
        );
    }
    let tracks = Table::new()
        .accessibility_label("Album tracks")
        .child(
            TableHeader::new().child(
                TableRow::new()
                    .child(TableHead::new().w(px(44.)).child("#"))
                    .child(TableHead::new().flex_1().child("Title"))
                    .child(TableHead::new().w(px(64.)).text_right().child("Length")),
            ),
        )
        .child(body);
    sheet.title(album.title.clone()).size(px(480.)).child(
        div()
            .v_flex()
            .gap_5()
            .child(facts)
            .child(tracks)
            .overflow_y_scrollbar(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AlbumRow, ColumnSort, LibraryColumn, LibraryTable, Track, duration, playtime, read,
    };
    use muzik_library::{Fields, Library, SqlValue};
    use std::sync::Arc;

    fn album(artist: &str, title: &str, year: i64, added: f64) -> AlbumRow {
        AlbumRow {
            artist: artist.into(),
            title: title.into(),
            year,
            added,
            ..AlbumRow::default()
        }
    }

    #[test]
    fn read_groups_tracks_under_albums_and_counts_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("library.db");
        let fields = |pairs: &[(&str, SqlValue)]| -> Fields {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect()
        };
        Library::open_or_create(&database)
            .unwrap()
            .transaction(|library| {
                let album_id = library.insert_album(
                    &fields(&[
                        ("albumartist", SqlValue::Text("Phoniks".into())),
                        ("album", SqlValue::Text("Down To Earth".into())),
                        ("year", SqlValue::Integer(2015)),
                        ("added", SqlValue::Real(1_700_000_000.)),
                    ]),
                    &Fields::new(),
                )?;
                for (number, title, length) in [(2, "Second", 125.4), (1, "First", 60.)] {
                    library.insert_item(
                        &fields(&[
                            ("album_id", SqlValue::Integer(album_id)),
                            ("track", SqlValue::Integer(number)),
                            ("title", SqlValue::Text(title.into())),
                            ("artist", SqlValue::Text("Phoniks".into())),
                            ("length", SqlValue::Real(length)),
                            ("format", SqlValue::Text("FLAC".into())),
                            (
                                "path",
                                SqlValue::Text(format!("/Music/Phoniks/{title}.flac")),
                            ),
                        ]),
                        &Fields::new(),
                    )?;
                }
                library.insert_item(
                    &fields(&[
                        ("title", SqlValue::Text("Loose".into())),
                        ("artist", SqlValue::Text("Awon".into())),
                        ("length", SqlValue::Real(30.)),
                        ("format", SqlValue::Text("mp3".into())),
                    ]),
                    &Fields::new(),
                )?;
                Ok(())
            })
            .unwrap();
        let info = read(&database, dir.path()).unwrap();
        assert_eq!(info.tracks, 3);
        assert_eq!(info.singletons, 1);
        assert_eq!(info.artists, 2);
        assert_eq!(info.albums.len(), 1);
        let row = &info.albums[0];
        assert_eq!(
            row.tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
        assert_eq!(duration(row.length()), "3:05");
        assert_eq!(row.format(), "FLAC");
        assert_eq!(row.path, "/Music/Phoniks");
        assert_eq!(
            info.formats,
            [("FLAC".to_string(), 2), ("MP3".to_string(), 1)]
        );
    }

    #[test]
    fn durations_read_as_clock_time_and_playtime() {
        assert_eq!(duration(59.6), "1:00");
        assert_eq!(duration(3725.), "1:02:05");
        assert_eq!(playtime(5400.), "1 h 30 min");
        assert_eq!(playtime(90_000.), "1 d 1 h");
    }

    #[test]
    fn the_filter_and_sort_choose_the_shown_albums() {
        let mut table = LibraryTable::new();
        let mut older = album("Tommy Guerrero", "Road to Knowhere", 2011, 1.);
        older.tracks = vec![Track::default(); 3];
        table.set_albums(
            [
                older,
                album("Phoniks", "Down To Earth", 2015, 3.),
                album("Awon & Phoniks", "The Actual Proof", 2016, 2.),
            ]
            .map(Arc::new)
            .to_vec(),
        );
        let titles = |table: &LibraryTable| {
            (0..table.shown())
                .filter_map(|row| table.album(row).map(|album| album.title.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(&table),
            ["Road to Knowhere", "Down To Earth", "The Actual Proof"]
        );
        table.set_filter(" PHONIKS ");
        assert_eq!(titles(&table), ["Down To Earth", "The Actual Proof"]);
        table.set_filter("2011");
        assert_eq!(titles(&table), ["Road to Knowhere"]);
        table.set_filter("");
        table.sort = Some((LibraryColumn::Artist, ColumnSort::Ascending));
        table.apply();
        assert_eq!(
            titles(&table),
            ["The Actual Proof", "Down To Earth", "Road to Knowhere"]
        );
    }
}
