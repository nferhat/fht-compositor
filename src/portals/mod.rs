use anyhow::Context;
use smithay::reexports::calloop::{self, LoopHandle};

use crate::state::State;
use crate::utils::dbus::DBUS_CONNECTION;

#[cfg(feature = "xdg-screencast-portal")]
mod screencast;
#[cfg(feature = "xdg-screencast-portal")]
#[allow(unused_imports)]
pub use screencast::{
    CursorMode, Request as ScreenCastRequest, Response as ScreenCastResponse, SessionSource,
    SourceType, PORTAL_VERSION,
};

pub fn start(loop_handle: &LoopHandle<'static, State>) -> anyhow::Result<()> {
    #[cfg(feature = "xdg-screencast-portal")]
    {
        info!("Starting XDG screencast portal!");
        let (to_compositor, from_screencast) = calloop::channel::channel::<ScreenCastRequest>();
        let (to_screencast, from_compositor) =
            async_std::channel::unbounded::<ScreenCastResponse>();
        let portal = screencast::Portal {
            from_compositor,
            to_compositor: to_compositor.clone(),
        };
        loop_handle
            .insert_source(from_screencast, move |event, _, state| {
                let calloop::channel::Event::Msg(req) = event else {
                    return;
                };
                state.handle_screencast_request(req, &to_screencast, &to_compositor);
            })
            .map_err(|err| {
                anyhow::anyhow!("Failed to insert XDG screencast portal source! {err}")
            })?;
        assert!(DBUS_CONNECTION
            .object_server()
            .at("/org/freedesktop/portal/desktop", portal)
            .context("Failed to insert XDG screencast portal in dbus!")?);
    }

    Ok(())
}
