//! Purchased items and collection pages.

use crate::{Login, PAGE_LIMIT, Result, USER_AGENT};
use serde_json::{Value, json};
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
    let mut urls = data("redownload_urls")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut more = data("item_count").and_then(Value::as_u64).unwrap_or(0)
        > data("batch_size").and_then(Value::as_u64).unwrap_or(0);
    let mut token = data("last_token").cloned().unwrap_or(Value::Null);
    while more {
        let mut response = ureq::post("https://bandcamp.com/api/fancollection/1/collection_items")
            .header("User-Agent", USER_AGENT)
            .header("Cookie", login.header())
            .config()
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
        details.extend(
            page.get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        if let Some(page_urls) = page.get("redownload_urls").and_then(Value::as_object) {
            urls.extend(page_urls.clone());
        }
        more = page.get("more_available") == Some(&Value::Bool(true));
        token = page.get("last_token").cloned().unwrap_or(Value::Null);
    }
    Ok(purchases(&details, &urls))
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
}
