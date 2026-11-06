//! Purchased items and collection pages.

use crate::{Login, PAGE_LIMIT, Result, USER_AGENT};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Purchase {
    pub key: String,
    pub artist: String,
    pub title: String,
    pub single: bool,
    pub download_page: String,
    pub item_url: Option<String>,
    pub art_url: Option<String>,
}

impl Purchase {
    #[must_use]
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// # Errors
/// Returns an error when Bandcamp does not answer, does not accept the login, or sends data that is not valid.
pub fn collection(login: &Login) -> Result<Vec<Purchase>> {
    let blob = login.page_blob(&format!("https://bandcamp.com/{}", login.user))?;
    if blob.pointer("/fan_data/is_own_page") != Some(&Value::Bool(true)) {
        return Err(format!(
            "Bandcamp did not accept the login for \"{}\". Check the user name, or copy the cookies again.",
            login.user
        )
        .into());
    }
    let fan_id = blob
        .pointer("/fan_data/fan_id")
        .cloned()
        .unwrap_or(Value::Null);
    let mut pages = CollectionPages::new(&blob)?;
    while let Some(token) = pages.next.clone() {
        let mut response = ureq::post("https://bandcamp.com/api/fancollection/1/collection_items")
            .header("User-Agent", USER_AGENT)
            .header("Cookie", login.header())
            .config()
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .send_json(json!({"fan_id": fan_id, "older_than_token": token}))
            .map_err(|error| format!("Bandcamp did not send the next collection page: {error}"))?;
        let page: Value = response
            .body_mut()
            .with_config()
            .limit(PAGE_LIMIT)
            .read_json()
            .map_err(|error| {
                format!("Bandcamp sent a collection page that is not valid: {error}")
            })?;
        pages.append(page)?;
    }
    Ok(purchases(&pages.details, &pages.urls))
}

#[derive(Deserialize)]
struct CollectionPage {
    items: Vec<Value>,
    more_available: bool,
    last_token: Option<String>,
    redownload_urls: Option<Map<String, Value>>,
}

struct CollectionPages {
    details: Vec<Value>,
    urls: Map<String, Value>,
    next: Option<String>,
    seen: HashSet<String>,
}

impl CollectionPages {
    fn new(blob: &Value) -> Result<Self> {
        let data = |key: &str| {
            blob.get("collection_data")
                .and_then(|collection| collection.get(key))
        };
        let mut details: Vec<Value> = Vec::new();
        let cache = blob
            .pointer("/item_cache/collection")
            .and_then(Value::as_object);
        let sequence = data("sequence").and_then(Value::as_array);
        match (sequence, cache) {
            (Some(sequence), Some(cache)) => details.extend(
                sequence
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|key| cache.get(key).cloned()),
            ),
            (None, Some(cache)) => details.extend(cache.values().cloned()),
            _ => {}
        }
        let urls = data("redownload_urls")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let more = data("item_count").and_then(Value::as_u64).unwrap_or(0)
            > data("batch_size").and_then(Value::as_u64).unwrap_or(0);
        let token = data("last_token").cloned().unwrap_or(Value::Null);
        let mut pages = Self {
            details,
            urls,
            next: None,
            seen: HashSet::new(),
        };
        pages.set_next(more, token.as_str())?;
        Ok(pages)
    }

    fn set_next(&mut self, more: bool, token: Option<&str>) -> Result<()> {
        self.next = if more {
            let token = token
                .filter(|token| !token.trim().is_empty())
                .ok_or("Bandcamp sent a continuing collection page without a token.")?;
            if !self.seen.insert(token.to_owned()) {
                return Err("Bandcamp sent a repeated collection page token.".into());
            }
            Some(token.to_owned())
        } else {
            None
        };
        Ok(())
    }

    fn append(&mut self, page: Value) -> Result<()> {
        let page: CollectionPage = serde_json::from_value(page).map_err(|error| {
            format!("Bandcamp sent a collection page that is not valid: {error}")
        })?;
        self.set_next(page.more_available, page.last_token.as_deref())?;
        self.details.extend(page.items);
        self.urls.extend(page.redownload_urls.unwrap_or_default());
        Ok(())
    }
}

fn purchases(details: &[Value], urls: &serde_json::Map<String, Value>) -> Vec<Purchase> {
    let mut seen = std::collections::HashSet::new();
    details
        .iter()
        .filter_map(|item| {
            let key = format!(
                "{}{}",
                item["sale_item_type"].as_str()?,
                item["sale_item_id"]
            );
            let download_page = urls.get(&key)?.as_str()?.to_owned();
            seen.insert(key.clone()).then(|| Purchase {
                artist: item["band_name"].as_str().unwrap_or("").to_owned(),
                title: item["item_title"]
                    .as_str()
                    .or_else(|| item["album_title"].as_str())
                    .unwrap_or(&key)
                    .to_owned(),
                single: item["tralbum_type"] == "t" || item["item_type"] == "track",
                download_page,
                item_url: item["item_url"].as_str().map(str::to_owned),
                art_url: item["item_art_id"]
                    .as_u64()
                    .map(|id| format!("https://f4.bcbits.com/img/a{id:010}_9.jpg")),
                key,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn purchases_follow_the_collection_order_and_need_a_download_page() {
        let details = vec![
            json!({"sale_item_type": "p", "sale_item_id": 2, "band_name": "Band", "item_title": "Album", "tralbum_type": "a", "item_art_id": 123}),
            json!({"sale_item_type": "p", "sale_item_id": 1, "band_name": "Band", "item_title": "Song", "tralbum_type": "t"}),
            json!({"sale_item_type": "p", "sale_item_id": 3, "band_name": "Band", "item_title": "Gift"}),
        ];
        let mut urls = serde_json::Map::new();
        urls.insert("p1".into(), json!("https://bandcamp.com/download?id=1"));
        urls.insert("p2".into(), json!("https://bandcamp.com/download?id=2"));
        let list = purchases(&details, &urls);
        assert_eq!(
            list.iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            ["p2", "p1"]
        );
        assert_eq!(list[0].label(), "Band - Album");
        assert!(!list[0].single);
        assert!(list[1].single);
        assert_eq!(
            list[0].art_url.as_deref(),
            Some("https://f4.bcbits.com/img/a0000000123_9.jpg")
        );
    }
    #[test]
    fn collection_pages_require_items_and_a_boolean_continuation_flag() {
        for page in [
            json!({}),
            json!({"items": []}),
            json!({"more_available": false}),
            json!({"items": null, "more_available": false}),
            json!({"items": {}, "more_available": false}),
            json!({"items": [], "more_available": "false"}),
        ] {
            let mut pages = CollectionPages::new(&json!({})).unwrap();
            let error = pages.append(page).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("collection page that is not valid")
            );
        }
    }

    #[test]
    fn continuing_collection_pages_need_new_nonempty_tokens() {
        let initial =
            json!({"collection_data": {"item_count": 2, "batch_size": 1, "last_token": "first"}});
        for token in [Value::Null, json!(""), json!(" "), json!(4), json!("first")] {
            let mut pages = CollectionPages::new(&initial).unwrap();
            assert!(
                pages
                    .append(json!({"items": [], "more_available": true, "last_token": token}))
                    .is_err()
            );
        }
        let mut missing = initial.clone();
        missing["collection_data"]
            .as_object_mut()
            .unwrap()
            .remove("last_token");
        assert!(CollectionPages::new(&missing).is_err());
        let mut pages = CollectionPages::new(&initial).unwrap();
        pages
            .append(json!({"items": [], "more_available": true, "last_token": "second"}))
            .unwrap();
        let error = pages
            .append(json!({"items": [], "more_available": true, "last_token": "first"}))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Bandcamp sent a repeated collection page token."
        );
    }

    #[test]
    fn collection_pages_keep_purchase_order_and_end_without_a_token() {
        let first = json!({"sale_item_type": "p", "sale_item_id": 1, "item_title": "First"});
        let mut pages = CollectionPages::new(&json!({
            "collection_data": {"item_count": 2, "batch_size": 1, "last_token": "first", "sequence": ["p1"], "redownload_urls": {"p1": "https://bandcamp.com/download/1"}},
            "item_cache": {"collection": {"p1": first}}
        })).unwrap();
        assert_eq!(pages.next.as_deref(), Some("first"));
        pages.append(json!({"items": [{"sale_item_type": "p", "sale_item_id": 2, "item_title": "Second"}], "more_available": false, "redownload_urls": {"p2": "https://bandcamp.com/download/2"}})).unwrap();
        assert_eq!(pages.next, None);
        let list = purchases(&pages.details, &pages.urls);
        assert_eq!(
            list.iter()
                .map(|purchase| purchase.title.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
    }
}
