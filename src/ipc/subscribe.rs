//! Subscribing functionality for `fht-compositor`
//!
//! Most of the code has been written by @Byson94! Thank you very much

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;

use async_broadcast::Receiver;
use calloop::io::Async;
use futures_util::io::WriteHalf;
use futures_util::AsyncWriteExt;

/// Compositor state to track between the IPC server and each client.
#[derive(Default, Clone, Debug)]
pub struct CompositorState {
    pub windows: HashMap<usize, fht_compositor_ipc::Window>,
    pub workspaces: HashMap<usize, fht_compositor_ipc::Workspace>,
    pub space: fht_compositor_ipc::Space,
    pub layer_shells: Vec<fht_compositor_ipc::LayerShell>,
}

/// Start a subscription for a given [`UnixStream`].
///
/// While the channel is active, or the stream didn't disconnect yet, we keep receiving events and
/// writing them to the subscribed client.
pub(super) async fn start_subscribing(
    mut rx: Receiver<fht_compositor_ipc::Event>,
    initial_state: async_channel::Receiver<CompositorState>,
    mut writer: WriteHalf<Async<'static, UnixStream>>,
) -> anyhow::Result<()> {
    let CompositorState {
        windows,
        workspaces,
        space,
        layer_shells,
    } = match initial_state.recv().await {
        Ok(s) => s,
        Err(err) => anyhow::bail!("Failed to receive initial state: {err:?}"),
    };

    let initial_events = [
        fht_compositor_ipc::Event::Windows(windows),
        fht_compositor_ipc::Event::Workspaces(workspaces),
        fht_compositor_ipc::Event::Space(space),
        fht_compositor_ipc::Event::LayerShells(layer_shells),
    ];
    for event in initial_events {
        let mut json_string = serde_json::to_string(&event).unwrap();
        json_string.push('\n');
        if let Err(err) = writer.write_all(json_string.as_bytes()).await {
            anyhow::bail!("Failed to communicate initial state to client: {err:?}");
        }
    }

    while let Ok(event) = rx.recv().await {
        let mut json_string = serde_json::to_string(&event).unwrap();
        json_string.push('\n');
        match writer.write_all(json_string.as_bytes()).await {
            Ok(()) => (),
            // Client disconnected, stop this thread.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
            Err(err) => anyhow::bail!("Failed to communicate initial state to client: {err:?}"),
        }
    }

    return Ok(());
}
