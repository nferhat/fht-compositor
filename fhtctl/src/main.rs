use std::process;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use fhtctl::client::Client;

#[derive(Debug, Parser)]
#[command(name = "fhtctl", about = "Control fht-compositor via IPC")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Get compositor state information (version and uptime)
    State,

    /// List all active outputs (monitors)
    Outputs,

    /// List all workspaces
    Workspaces,

    /// List all windows
    Windows,

    /// Focus a window by app_id and/or title
    #[command(arg_required_else_help = true)]
    Focus {
        #[arg(long)]
        app_id: Option<String>,

        #[arg(long)]
        title: Option<String>,
    },

    /// Close the currently focused window
    Close,

    /// Switch to the specified workspace
    #[command(arg_required_else_help = true)]
    Workspace {
        id: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut client = match Client::connect() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("Error connecting to fht-compositor: {}", e);
            eprintln!("Is the compositor running?");
            process::exit(1);
        }
    };

    let response = match cli.command {
        Commands::State => client.get_state()?,
        Commands::Outputs => client.get_outputs()?,
        Commands::Workspaces => client.get_workspaces()?,
        Commands::Windows => client.get_windows()?,
        Commands::Focus { app_id, title } => {
            if app_id.is_none() && title.is_none() {
                return Err(anyhow!("Either --app-id or --title must be specified"));
            }
            client.focus_window(app_id, title)?
        }
        Commands::Close => client.close_window()?,
        Commands::Workspace { id } => client.switch_workspace(id)?,
    };

    if !response.success {
        eprintln!(
            "Error: {}",
            response
                .message
                .unwrap_or_else(|| "Unknown error".to_string())
        );
        process::exit(1);
    }

    if let Some(data) = response.data {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else if let Some(message) = response.message {
        println!("{}", message);
    } else {
        println!("Success");
    }

    Ok(())
}
