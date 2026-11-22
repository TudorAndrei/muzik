use futures_util::{Stream, stream};
use muzik_runner::service::Service;
use serde_json::{Value, json};
use std::time::Duration;

use tokio::{sync::broadcast::error::RecvError, time::MissedTickBehavior};
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

const SESSION_CHECK: Duration = Duration::from_secs(1);

#[route(GET "/api/jobs")]
pub async fn jobs(cx: &Cx) -> Result<Json<Value>> {
    auth::require(cx).await?;
    Ok(Json(backend(cx).run(Service::jobs).await?))
}

#[route(GET "/api/events")]
pub async fn events_stream(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    auth::require(cx).await?;
    let watch = auth::watch(cx).await?;
    let receiver = events(cx).0.subscribe();
    let mut check = tokio::time::interval(SESSION_CHECK);
    check.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let stream = stream::unfold(
        (receiver, watch, check),
        |(mut receiver, watch, mut check)| async move {
            loop {
                tokio::select! {
                    next = receiver.recv() => {
                        if !watch.is_active() {
                            return None;
                        }
                        match next {
                            Ok(event) => {
                                return Some((
                                    Event::new().json_data(&event),
                                    (receiver, watch, check),
                                ));
                            }
                            Err(RecvError::Lagged(_)) => {
                                return Some((
                                    Event::new().json_data(&json!({"type": "resync"})),
                                    (receiver, watch, check),
                                ));
                            }
                            Err(RecvError::Closed) => return None,
                        }
                    }
                    _ = check.tick() => {
                        if !watch.is_active() {
                            return None;
                        }
                    }
                }
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::new()))
}
