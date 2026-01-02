use anyhow::Context;
use smithay::reexports::calloop::{self, LoopHandle};

use crate::state::State;

mod shared;

#[cfg(feature = "xdg-global-shortcuts-portal")]
pub mod global_shortcuts;
#[cfg(feature = "xdg-screencast-portal")]
pub mod screencast;

pub fn start(
    dbus_connection: &zbus::blocking::Connection,
    loop_handle: &LoopHandle<'static, State>,
) -> anyhow::Result<()> {
    #[cfg(feature = "xdg-screencast-portal")]
    {
        info!("Starting XDG screencast portal");
        let (to_compositor, from_screencast) = calloop::channel::channel::<screencast::Request>();
        let portal = screencast::Portal::new(to_compositor);
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
        assert!(dbus_connection
            .object_server()
            .at("/org/freedesktop/portal/desktop", portal)
            .context("Failed to insert XDG screencast portal in dbus!")?);
    }

    #[cfg(feature = "xdg-global-shortcuts-portal")]
    {
        info!("Starting global-shortcuts portal");
        let (to_compositor, from_screencast) = calloop::channel::channel();
        let portal = global_shortcuts::Portal::new(to_compositor);
        loop_handle
            .insert_source(from_screencast, move |event, _, state| {
                let calloop::channel::Event::Msg(req) = event else {
                    return;
                };
                _ = (req, state);
            })
            .map_err(|err| {
                anyhow::anyhow!("Failed to insert XDG screencast portal source! {err}")
            })?;
        assert!(dbus_connection
            .object_server()
            .at("/org/freedesktop/portal/desktop", portal)
            .context("Failed to insert XDG screencast portal in dbus!")?);
    }

    Ok(())
}
