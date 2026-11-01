use serde_json::Value;

static NULL: Value = Value::Null;

pub fn list(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split(", ").map(str::to_owned).collect()
}

pub fn field<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value.pointer(pointer).unwrap_or(&NULL)
}

pub fn playlist<'a>(document: &'a Value, id: &str) -> anyhow::Result<&'a Value> {
    document
        .get("playlists")
        .and_then(Value::as_array)
        .and_then(|playlists| {
            playlists
                .iter()
                .find(|playlist| playlist.get("playlist_id").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| anyhow::anyhow!("the watchlist has no source {id}: {document}"))
}

pub fn items<'a>(document: &'a Value, id: &str) -> anyhow::Result<&'a [Value]> {
    Ok(playlist(document, id)?
        .get("items")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice))
}

pub fn item<'a>(document: &'a Value, id: &str, ordinal: usize) -> anyhow::Result<&'a Value> {
    ordinal
        .checked_sub(1)
        .and_then(|index| items(document, id).ok()?.get(index))
        .ok_or_else(|| anyhow::anyhow!("the source {id} has no item {ordinal}: {document}"))
}

pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
