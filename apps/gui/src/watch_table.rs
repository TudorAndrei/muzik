use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::tag::Tag;

fn summary_tag(summary: Summary) -> Tag {
    match summary {
        Summary::Pending => Tag::secondary(),
        Summary::Processing => Tag::info(),
        Summary::Waiting => Tag::warning(),
        Summary::Failed => Tag::danger(),
        Summary::Processed => Tag::success(),
        Summary::Unavailable => Tag::secondary().outline(),
    }
}

pub struct WatchRow {
    pub key: ItemId,
    pub item: ItemView,
    pub queued: bool,
}

impl WatchRow {
    fn title(&self) -> &str {
        &self.item.item.title
    }
    fn summary(&self) -> &str {
        self.item.summary.as_ref()
    }
    fn error(&self) -> &str {
        self.item.item.last_error.as_deref().unwrap_or("")
    }
    fn primary(&self) -> Option<(ItemAction, String, bool)> {
        let primary = self.item.primary_action.as_ref()?;
        let enabled = self
            .item
            .actions
            .get(&primary.action)
            .is_some_and(|availability| availability.enabled);
        Some((
            primary.action,
            primary.label.clone(),
            enabled && !self.queued,
        ))
    }
}

const COLUMNS: [(&str, &str, f32); 6] = [
    ("position", "#", 56.),
    ("title", "Title", 340.),
    ("stages", "Stages", 110.),
    ("status", "Status", 110.),
    ("action", "", 90.),
    ("error", "Error", 420.),
];
const SORTABLE: [usize; 3] = [0, 1, 3];

pub struct WatchTable {
    rows: Vec<WatchRow>,
    sort: Option<(usize, ColumnSort)>,
    view: WeakEntity<Muzik>,
}

impl WatchTable {
    pub(crate) const fn new(view: WeakEntity<Muzik>) -> Self {
        Self {
            rows: Vec::new(),
            sort: None,
            view,
        }
    }

    pub(crate) fn set_rows(&mut self, rows: Vec<WatchRow>) {
        self.rows = rows;
        self.apply_sort();
    }

    pub(crate) fn key(&self, row_ix: usize) -> Option<ItemId> {
        self.rows.get(row_ix).map(|row| row.key.clone())
    }

    fn apply_sort(&mut self) {
        let (column, order) = self.sort.unwrap_or((0, ColumnSort::Default));
        match column {
            1 => self
                .rows
                .sort_by_cached_key(|row| row.title().to_lowercase()),
            3 => self.rows.sort_by(|left, right| {
                left.summary()
                    .cmp(right.summary())
                    .then(left.key.position.cmp(&right.key.position))
            }),
            _ => self.rows.sort_by_key(|row| row.key.position),
        }
        if order == ColumnSort::Descending {
            self.rows.reverse();
        }
    }
}

fn run_item(view: &WeakEntity<Muzik>, key: &ItemId, action: ItemAction, cx: &mut App) {
    let _ = view.update(cx, |view, cx| {
        let item = view.item_request(key.clone(), action);
        view.run_item(item, cx);
    });
}

pub fn open_item(view: &WeakEntity<Muzik>, key: ItemId, window: &mut Window, cx: &mut App) {
    let _ = view.update(cx, |view, cx| view.open_item_sheet(key, window, cx));
}

impl TableDelegate for WatchTable {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let Some(&(key, name, width)) = COLUMNS.get(col_ix) else {
            return Column::new("", "");
        };
        let column = Column::new(key, name).width(px(width));
        if SORTABLE.contains(&col_ix) {
            column.sortable()
        } else {
            column
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = Some((col_ix, sort));
        self.apply_sort();
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        match col_ix {
            0 => div().child(row.key.position.to_string()).into_any_element(),
            1 => div()
                .min_w_0()
                .truncate()
                .child(row.title().to_string())
                .into_any_element(),
            2 => style::stage_track(
                SharedString::from(format!("watch-stages-{}", row.key)),
                &row.item.item,
                cx,
            ),
            3 => {
                let tag = if row.queued {
                    Tag::info().outline().child("In the queue")
                } else {
                    summary_tag(row.item.summary).child(row.summary().to_string())
                };
                div().flex().child(tag.small()).into_any_element()
            }
            5 => div()
                .min_w_0()
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .child(row.error().to_string())
                .into_any_element(),
            _ => match row.primary() {
                Some((action, label, enabled)) => {
                    let key = row.key.clone();
                    let view = self.view.clone();
                    Button::new(SharedString::from(format!("watch-action-{key}")))
                        .xsmall()
                        .label(label)
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| run_item(&view, &key, action, cx))
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
        }
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(row) = self.rows.get(row_ix) else {
            return menu;
        };
        let key = row.key.clone();
        let view = self.view.clone();
        let details = (key.clone(), view.clone());
        let mut menu = menu.item(
            PopupMenuItem::new("Details").on_click(move |_, window, cx| {
                open_item(&details.1, details.0.clone(), window, cx);
            }),
        );
        if let Some((action, label, enabled)) = row.primary() {
            menu = menu.item(
                PopupMenuItem::new(label)
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| run_item(&view, &key, action, cx)),
            );
        }
        menu
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        let Some(row) = self.rows.get(row_ix) else {
            return String::new();
        };
        match col_ix {
            0 => row.key.position.to_string(),
            1 => row.title().to_string(),
            3 => row.summary().to_string(),
            5 => row.error().to_string(),
            _ => String::new(),
        }
    }
}

pub fn rows(
    playlist: &PlaylistView,
    filter: Option<Summary>,
    queued: &HashSet<String>,
) -> Vec<WatchRow> {
    playlist
        .items
        .iter()
        .filter(|item| watchlist_view::matches_filter(item, filter))
        .map(|item| {
            let key = ItemId::of(&playlist.playlist_id, &item.item);
            let queued = queued.contains(&key.to_string());
            WatchRow {
                key,
                item: item.clone(),
                queued,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ColumnSort, WatchTable, rows};
    use gpui_kit::WeakEntity;
    use muzik_store::watchlist::Summary;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn rows_follow_the_filter_and_mark_queued_items() {
        let playlist: super::PlaylistView = serde_json::from_value(json!({"playlist_id": "PL1", "url":"https://www.youtube.com/playlist?list=PL1", "items": [
            {"position": 1, "title": "One", "video_id": "aaaaaaaaaaa", "actions":{}, "summary": "Failed"},
            {"position": 2, "title": "Two", "video_id": "bbbbbbbbbbb", "actions":{}, "summary": "Processed"},
            {"position": 3, "title": "Three", "video_id": "ccccccccccc", "actions":{}, "summary": "Failed"},
            {"position": 4, "title": "Gone", "video_id": "ddddddddddd", "actions":{}, "summary": "Unavailable"}
        ]})).unwrap();
        let queued =
            HashSet::from([
                muzik_store::watchlist::ItemId::new("PL1", 3, Some("ccccccccccc")).to_string(),
            ]);
        let list = rows(&playlist, Some(Summary::Failed), &queued);
        assert_eq!(
            list.iter().map(|row| row.key.position).collect::<Vec<_>>(),
            [1, 3]
        );
        assert!(!list[0].queued);
        assert!(list[1].queued);
        let all = rows(&playlist, None, &queued);
        assert_eq!(
            all.iter().map(|row| row.key.position).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        let gone = rows(&playlist, Some(Summary::Unavailable), &queued);
        assert_eq!(
            gone.iter().map(|row| row.key.position).collect::<Vec<_>>(),
            [4]
        );
    }

    #[test]
    fn sorting_by_title_and_status_keeps_a_stable_order() {
        let mut table = WatchTable::new(WeakEntity::new_invalid());
        let playlist: super::PlaylistView = serde_json::from_value(json!({"playlist_id": "PL1", "url":"https://www.youtube.com/playlist?list=PL1", "items": [
            {"position": 1, "title": "beta", "video_id": "a", "actions":{}, "summary": "Pending"},
            {"position": 2, "title": "Alpha", "video_id": "b", "actions":{}, "summary": "Failed"},
            {"position": 3, "title": "gamma", "video_id": "c", "actions":{}, "summary": "Failed"}
        ]})).unwrap();
        table.set_rows(rows(&playlist, None, &HashSet::new()));
        table.sort = Some((1, ColumnSort::Ascending));
        table.apply_sort();
        assert_eq!(
            table
                .rows
                .iter()
                .map(super::WatchRow::title)
                .collect::<Vec<_>>(),
            ["Alpha", "beta", "gamma"]
        );
        table.sort = Some((3, ColumnSort::Descending));
        table.apply_sort();
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.key.position)
                .collect::<Vec<_>>(),
            [1, 3, 2]
        );
        table.sort = Some((0, ColumnSort::Default));
        table.apply_sort();
        assert_eq!(
            table
                .rows
                .iter()
                .map(|row| row.key.position)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }
}
