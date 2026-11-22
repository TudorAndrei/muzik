use std::sync::Arc;

use muzik_runner::{AppEvent, service::Service};
use tokio::sync::broadcast;
use topcoat::{
    Result,
    context::{Cx, context},
};

use crate::hub::Hub;

pub const EVENT_BUFFER: usize = 1024;

#[derive(Clone)]
pub struct Backend(pub Arc<Service>);

impl Backend {
    /// # Errors
    /// Returns an error when the blocking task fails to finish.
    pub async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Service) -> T + Send + 'static,
    ) -> Result<T> {
        let service = Arc::clone(&self.0);
        Ok(tokio::task::spawn_blocking(move || work(&service)).await?)
    }
}

#[derive(Clone)]
pub struct Events(pub broadcast::Sender<AppEvent>);

#[derive(Clone)]
pub struct Shared(pub Arc<Hub>);

#[must_use]
pub fn backend(cx: &Cx) -> &Backend {
    context::<Backend>(cx)
}

#[must_use]
pub fn events(cx: &Cx) -> &Events {
    context::<Events>(cx)
}

#[must_use]
pub fn shared_hub(cx: &Cx) -> Arc<Hub> {
    Arc::clone(&context::<Shared>(cx).0)
}

#[must_use]
pub fn hub(cx: &Cx) -> &Hub {
    &context::<Shared>(cx).0
}
