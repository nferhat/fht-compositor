use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use smithay::reexports::calloop::{self, LoopHandle, RegistrationToken};
use smithay::reexports::rustix::path::Arg;

use crate::state::State;

pub mod ui;

pub struct Watcher {
    // This token is a handle to the calloop channel that drives the reload_config messages
    token: RegistrationToken,
    _join_handles: Vec<JoinHandle<()>>,
}

impl Watcher {
    pub fn stop(self, loop_handle: &LoopHandle<'static, State>) {
        loop_handle.remove(self.token);
    }
}

pub fn init_watcher(
    paths: Vec<PathBuf>,
    loop_handle: &LoopHandle<'static, State>,
) -> anyhow::Result<Watcher> {
    // We use a () as a dummy message to notify that the configuration file changed
    let (tx, channel) = calloop::channel::channel::<()>();
    let token = loop_handle
        .insert_source(channel, |event, _, state| {
            if let calloop::channel::Event::Msg(()) = event {
                state.reload_config()
            }
        })
        .map_err(|err| {
            anyhow::anyhow!("Failed to insert config file watcher source into event loop! {err}")
        })?;

    let mut handles = vec![];
    for path in paths {
        let tx = tx.clone();
        let path_2 = path.clone();
        match std::thread::Builder::new()
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
                        if tx.send(()).is_err() {
                            // Silently error as this is a way to stop this thread.
                            // The only possible error here is that the channel got dropped, in this
                            // case a new config file watcher will be created
                            break;
                        }
                    }
                }
            }) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                error!(
                    ?err,
                    path = ?path_2,
                    "Failed to start config file watcher for path"
                );
            }
        }
    }

    Ok(Watcher {
        token,
        _join_handles: handles,
    })
}
