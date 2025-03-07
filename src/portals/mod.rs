use anyhow::Context;
use smithay::reexports::calloop::{self, LoopHandle};

use crate::state::State;
use crate::utils::dbus::DBUS_CONNECTION;

mod shared;

#[cfg(feature = "xdg-screencast-portal")]
pub mod screencast;

pub fn start(loop_handle: &LoopHandle<'static, State>) -> anyhow::Result<()> {
    #[cfg(feature = "xdg-screencast-portal")]
    {
        info!("Starting XDG screencast portal");
        let (to_compositor, from_screencast) = calloop::channel::channel::<screencast::Request>();
        let portal = screencast::Portal { to_compositor };
        loop_handle
            .insert_source(from_screencast, move |event, _, state| {
                let calloop::channel::Event::Msg(req) = event else {
                    return;
                };
                state.handle_screencast_request(req);
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
