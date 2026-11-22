use futures_util::{Stream, stream};
use muzik_runner::service::Service;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::{
            Json,
            sse::{Event, KeepAlive, Sse},
        },
        route,
    },
};

use crate::{
    auth,
    state::{backend, events},
};

#[route(GET "/api/jobs")]
pub async fn jobs(cx: &Cx) -> Result<Json<Value>> {
    auth::require(cx).await?;
    Ok(Json(backend(cx).run(Service::jobs).await?))
}

#[route(GET "/api/events")]
pub async fn events_stream(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    auth::require(cx).await?;
    let receiver = events(cx).0.subscribe();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    return Some((Event::new().json_data(&event), receiver));
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new()))
}
