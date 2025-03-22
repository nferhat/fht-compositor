use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod client;
pub mod server;

pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());

    Path::new(&runtime_dir).join("fht-compositor.sock")
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("Failed to connect to the compositor: {0}")]
    ConnectionFailed(#[from] std::io::Error),

    #[error("Invalid request format: {0}")]
    InvalidRequest(#[from] serde_json::Error),

    #[error("Request timed out")]
    Timeout,

    #[error("Unknown command: {0}")]
    UnknownCommand(String),

    #[error("The compositor returned an error: {0}")]
    CompositorError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Command {
    GetState,

    GetOutputs,

    GetWorkspaces,

    GetWindows,

    FocusWindow {
        app_id: Option<String>,
        title: Option<String>,
    },

    CloseWindow,

    SwitchWorkspace {
        id: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<ResponseData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResponseData {
    State { version: String, uptime: u64 },

    Outputs(Vec<Output>),

    Workspaces(Vec<Workspace>),

    Windows(Vec<Window>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
    pub make: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub refresh_rate: f64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: usize,
    pub name: String,
    pub active: bool,
    pub output: String,
    pub window_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub pid: Option<u32>,
    pub workspace_id: usize,
    pub focused: bool,
    pub maximized: bool,
    pub fullscreen: bool,
}

impl Response {
    pub fn success(data: Option<ResponseData>) -> Self {
        Self {
            success: true,
            message: None,
            data,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: None,
        }
    }
}
