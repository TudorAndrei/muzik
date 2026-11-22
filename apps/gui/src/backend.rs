use async_channel::Receiver;
use muzik_runner::AppEvent;
pub use muzik_runner::service::{ItemRequest, Service as Backend, SoulseekForm, SpotifyStatus};

pub fn start() -> muzik_runner::Result<(Backend, Receiver<AppEvent>)> {
    let paths = Backend::user_paths()?;
    let (sender, events) = async_channel::unbounded();
    let backend = Backend::start(
        paths,
        std::sync::Arc::new(move |event| {
            let _ = sender.try_send(event);
        }),
    )?;
    Ok((backend, events))
}
