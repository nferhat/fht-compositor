use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use anyhow::Result;

use crate::{socket_path, Command, IpcError, Response};

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect() -> Result<Self, IpcError> {
        let stream = UnixStream::connect(socket_path())?;
        Ok(Self { stream })
    }

    pub fn send_command(&mut self, command: Command) -> Result<Response, IpcError> {
        let command_json = serde_json::to_string(&command)?;

        self.stream.write_all(command_json.as_bytes())?;
        self.stream.write_all(b"\n")?;

        let mut response_data = String::new();
        self.stream.read_to_string(&mut response_data)?;

        let response: Response = serde_json::from_str(&response_data)?;

        Ok(response)
    }
}

impl Client {
    pub fn get_state(&mut self) -> Result<Response, IpcError> {
        self.send_command(Command::GetState)
    }

    pub fn get_outputs(&mut self) -> Result<Response, IpcError> {
        self.send_command(Command::GetOutputs)
    }

    pub fn get_workspaces(&mut self) -> Result<Response, IpcError> {
        self.send_command(Command::GetWorkspaces)
    }

    pub fn get_windows(&mut self) -> Result<Response, IpcError> {
        self.send_command(Command::GetWindows)
    }

    pub fn focus_window(
        &mut self,
        app_id: Option<String>,
        title: Option<String>,
    ) -> Result<Response, IpcError> {
        self.send_command(Command::FocusWindow { app_id, title })
    }

    pub fn close_window(&mut self) -> Result<Response, IpcError> {
        self.send_command(Command::CloseWindow)
    }

    pub fn switch_workspace(&mut self, id: usize) -> Result<Response, IpcError> {
        self.send_command(Command::SwitchWorkspace { id })
    }
}
