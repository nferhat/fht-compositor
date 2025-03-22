use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use async_channel::Sender;
use tracing::{error, info};

use crate::{socket_path, Command, Response};

pub struct Server {
    socket_path: PathBuf,
    callback_registry:
        Arc<Mutex<HashMap<Command, Box<dyn Fn(Command) -> Response + Send + 'static>>>>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            socket_path: socket_path(),
            callback_registry: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_handler<F>(&mut self, command_type: Command, callback: F)
    where
        F: Fn(Command) -> Response + Send + 'static,
    {
        let mut registry = self.callback_registry.lock().unwrap();
        registry.insert(command_type, Box::new(callback));
    }

    pub fn start(self) -> Result<ServerControl> {
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        let (tx, rx) = async_channel::bounded(1);

        let registry = self.callback_registry.clone();
        let socket_path = self.socket_path.clone();

        thread::spawn(move || {
            info!("IPC server started on {}", socket_path.display());

            listener.set_nonblocking(true).unwrap();

            loop {
                if rx.try_recv().is_ok() {
                    break;
                }

                match listener.accept() {
                    Ok((stream, _)) => {
                        let registry = registry.clone();
                        thread::spawn(move || {
                            if let Err(e) = handle_client(stream, registry) {
                                error!("Error handling client: {}", e);
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        error!("Error accepting connection: {}", e);
                        break;
                    }
                }
            }

            if socket_path.exists() {
                let _ = std::fs::remove_file(&socket_path);
            }

            info!("IPC server stopped");
        });

        Ok(ServerControl { tx })
    }
}

fn handle_client(
    mut stream: UnixStream,
    registry: Arc<Mutex<HashMap<Command, Box<dyn Fn(Command) -> Response + Send + 'static>>>>,
) -> Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut buffer = String::new();

    reader.read_line(&mut buffer)?;

    let command: Command = serde_json::from_str(&buffer)?;

    let registry = registry.lock().unwrap();

    let handler = registry.iter().find_map(|(registered_cmd, handler)| {
        if commands_match(registered_cmd, &command) {
            Some(handler)
        } else {
            None
        }
    });

    let response = match handler {
        Some(handler) => handler(command),
        None => {
            let cmd_name = format!("{:?}", command);
            Response::error(format!("Unknown command: {}", cmd_name))
        }
    };

    let response_json = serde_json::to_string(&response)?;
    stream.write_all(response_json.as_bytes())?;

    Ok(())
}

fn commands_match(cmd1: &Command, cmd2: &Command) -> bool {
    match (cmd1, cmd2) {
        (Command::GetState, Command::GetState) => true,
        (Command::GetOutputs, Command::GetOutputs) => true,
        (Command::GetWorkspaces, Command::GetWorkspaces) => true,
        (Command::GetWindows, Command::GetWindows) => true,
        (Command::CloseWindow, Command::CloseWindow) => true,
        (Command::FocusWindow { .. }, Command::FocusWindow { .. }) => true,
        (Command::SwitchWorkspace { .. }, Command::SwitchWorkspace { .. }) => true,
        _ => false,
    }
}

pub struct ServerControl {
    tx: Sender<()>,
}

impl ServerControl {
    pub async fn stop(self) -> Result<()> {
        self.tx.send(()).await?;
        Ok(())
    }
}
