use topcoat::{
    Result,
    asset::{AssetConfig, MANIFEST_VERSION, Manifest, ManifestEntry},
    context::Cx,
    router::{StatusCode, route},
    runtime::SCRIPT,
};

const FILE: &str = "topcoat-runtime.js";
const PREFIX: &str = "/_topcoat/assets";
const RUNTIME_SCRIPT: &str = include_str!("../assets/topcoat-runtime.js");

#[must_use]
pub fn config() -> AssetConfig {
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestEntry {
            id: SCRIPT.id(),
            file: FILE.to_owned(),
            hash: String::new(),
            content_type: "text/javascript".to_owned(),
        }],
    };
    AssetConfig::hosted_at(PREFIX, manifest)
}

#[route(GET "/_topcoat/assets/topcoat-runtime.js")]
pub async fn runtime_script(
    cx: &Cx,
) -> Result<(StatusCode, [(&'static str, &'static str); 2], &'static str)> {
    Ok((
        StatusCode::OK,
        [
            ("content-type", "text/javascript; charset=utf-8"),
            ("cache-control", "no-cache"),
        ],
        RUNTIME_SCRIPT,
    ))
}
