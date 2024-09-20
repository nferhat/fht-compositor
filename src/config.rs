use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use smithay::reexports::calloop::{self, LoopHandle, RegistrationToken};
use smithay::reexports::rustix::path::Arg;

use crate::state::State;

pub fn init_watcher(
    path: PathBuf,
    loop_handle: &LoopHandle<'static, State>,
) -> anyhow::Result<(RegistrationToken, JoinHandle<()>)> {
    // We use a () as a dummy message to notify that the configuration file changed
    let (tx, channel) = calloop::channel::channel::<()>();
    let join_handle: JoinHandle<()> = std::thread::Builder::new()
        .name(format!(
            "Config file watcher for: {}",
            path.as_str().unwrap()
        ))
        .spawn(move || {
            let path: &Path = path.as_ref();
            let mut last_mtime = path.metadata().and_then(|md| md.modified()).ok();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if let Some(new_mtime) = path
                    .metadata()
                    .and_then(|md| md.modified())
                    .ok()
                    .filter(|mtime| Some(mtime) != last_mtime.as_ref())
                {
                    debug!(?new_mtime, "Config file change detected");
                    last_mtime = Some(new_mtime);
                    if let Err(_) = tx.send(()) {
                        // Silently error as this is a way to stop this thread.
                        // The only possible error here is that the channel got dropped, in this
                        // case a new config file watcher will be created
                        break;
                    }
                }
            }
        })
        .context("Failed to start config file watcher thread")?;

    let token = loop_handle
        .insert_source(channel, |event, _, state| {
            if let calloop::channel::Event::Msg(()) = event {
                state.reload_config()
            }
        })
        .map_err(|err| {
            anyhow::anyhow!("Failed to insert config file watcher source into event loop! {err}")
        })?;

    Ok((token, join_handle))
}
