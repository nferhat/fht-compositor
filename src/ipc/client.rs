use std::io::{BufRead as _, BufReader, Write as _};

use fht_compositor_ipc::{PickLayerShellResult, PickWindowResult};

use crate::cli;

/// Make a single request to the running `fht-compositor` IPC server.
///
/// It uses the IPC socket specified by the `FHTC_SOCKET_PATH` environment variable.
pub fn make_request(request: cli::Request, json: bool) -> anyhow::Result<()> {
    let mut subscribe = false;
    let request = match request {
        cli::Request::Version => fht_compositor_ipc::Request::Version,
        cli::Request::Outputs => fht_compositor_ipc::Request::Outputs,
        cli::Request::Windows => fht_compositor_ipc::Request::Windows,
        cli::Request::Space => fht_compositor_ipc::Request::Space,
        cli::Request::Window { id } => fht_compositor_ipc::Request::Window(id),
        cli::Request::Workspace { id } => fht_compositor_ipc::Request::Workspace(id),
        cli::Request::GetWorkspace { output, index } => {
            fht_compositor_ipc::Request::GetWorkspace { output, index }
        }
        cli::Request::LayerShells => fht_compositor_ipc::Request::LayerShells,
        cli::Request::FocusedWindow => fht_compositor_ipc::Request::FocusedWindow,
        cli::Request::FocusedWorkspace => fht_compositor_ipc::Request::FocusedWorkspace,
        cli::Request::PickLayerShell => fht_compositor_ipc::Request::PickLayerShell,
        cli::Request::PickWindow => fht_compositor_ipc::Request::PickWindow,
        cli::Request::Action { action } => fht_compositor_ipc::Request::Action(action),
        cli::Request::CursorPosition => fht_compositor_ipc::Request::CursorPosition,
        cli::Request::PrintSchema => return fht_compositor_ipc::print_schema(),
        cli::Request::Subscribe => {
            subscribe = true;
            fht_compositor_ipc::Request::Subscribe
        }
    };

    // This is just a re-implementation of fht-compositor-ipc/test_client with cleaner error
    // handling for error logging yada-yada
    let (_, mut stream) = fht_compositor_ipc::connect()?;
    stream.set_nonblocking(false)?;

    let mut req = serde_json::to_string(&request).unwrap();
    req.push('\n'); // it is required to append a newline.
    stream.write_all(req.as_bytes())?;

    // If we are subscribing, keep reading events, and we won't write to this anymore
    if subscribe {
        // We must use a buf reader to get an entire line
        let mut buf_reader = BufReader::new(&mut stream);
        let mut stdout = std::io::stdout();
        loop {
            let mut res_buf = String::new();
            _ = buf_reader.read_line(&mut res_buf)?;
            stdout.write(res_buf.as_bytes())?;
        }
    }

    let mut res_buf = String::new();
    {
        // We must use a buf reader to get an entire line
        let mut buf_reader = BufReader::new(&mut stream);
        _ = buf_reader.read_line(&mut res_buf)
    }

    let response = serde_json::de::from_str(&res_buf)?;
    let response = match response {
        fht_compositor_ipc::Response::Error(err) => anyhow::bail!("IPC error: {err}"),
        res => res,
    };

    if json {
        let json_buffer = match response {
            fht_compositor_ipc::Response::Version(version) => {
                serde_json::to_string(&serde_json::json!({
                    "compositor": version,
                    "cli": crate::cli::get_version_string(),
                }))
            }
            fht_compositor_ipc::Response::Outputs(hash_map) => serde_json::to_string(&hash_map),
            fht_compositor_ipc::Response::Windows(windows) => serde_json::to_string(&windows),
            fht_compositor_ipc::Response::LayerShells(layer_shells) => {
                serde_json::to_string(&layer_shells)
            }
            fht_compositor_ipc::Response::Window(window) => serde_json::to_string(&window),
            fht_compositor_ipc::Response::Workspace(workspace) => {
                match &workspace {
                    // We unwrap the Some case in case the user asked for the FocusedWorkspace,
                    // which will always be present
                    Some(workspace) => serde_json::to_string(workspace),
                    none => serde_json::to_string(none),
                }
            }
            fht_compositor_ipc::Response::Space(space) => serde_json::to_string(&space),
            fht_compositor_ipc::Response::Error(err) => {
                anyhow::bail!("Failed to handle IPC request: {err:?}")
            }
            fht_compositor_ipc::Response::PickedLayerShell(result) => match result {
                PickLayerShellResult::Some(layer_shell) => serde_json::to_string(&layer_shell),
                PickLayerShellResult::None | PickLayerShellResult::Cancelled => {
                    Ok(serde_json::json!(null).to_string())
                }
            },
            fht_compositor_ipc::Response::PickedWindow(result) => match result {
                PickWindowResult::Some(window) => serde_json::to_string(&window),
                PickWindowResult::None | PickWindowResult::Cancelled => {
                    Ok(serde_json::json!(null).to_string())
                }
            },
            fht_compositor_ipc::Response::CursorPosition { x, y } => {
                Ok(serde_json::json!({ "x": x, "y": y }).to_string())
            }
            fht_compositor_ipc::Response::Noop => return Ok(()),
        }?;
        println!("{}", json_buffer);
        Ok(())
    } else {
        print_formatted(&response)?;
        Ok(())
    }
}

fn print_formatted(res: &fht_compositor_ipc::Response) -> anyhow::Result<()> {
    let mut writer = std::io::BufWriter::new(std::io::stdout());
    match res {
        fht_compositor_ipc::Response::Version(version) => {
            println!("Compositor: {version}");
            println!("CLI: {}", crate::cli::get_version_string())
        }
        fht_compositor_ipc::Response::Outputs(outputs) => {
            for (idx, output) in outputs.values().enumerate() {
                writeln!(&mut writer, "Output #{idx}: {}", output.name)?;
                writeln!(&mut writer, "\tName: {}", output.name)?;
                writeln!(&mut writer, "\tMake: {}", output.make)?;
                writeln!(&mut writer, "\tModel: {}", output.model)?;
                writeln!(&mut writer, "\tSerial: {:?}", output.serial)?;
                writeln!(
                    &mut writer,
                    "\tPhysical size (mm): {}",
                    if let Some((w, h)) = output.physical_size {
                        format!("({w}, {h})")
                    } else {
                        String::from("unknown")
                    }
                )?;
                writeln!(
                    &mut writer,
                    "\tLogical Position: ({}, {})",
                    output.position.0, output.position.1
                )?;
                writeln!(
                    &mut writer,
                    "\tSize (px): ({}, {})",
                    output.size.0, output.size.1
                )?;
                writeln!(&mut writer, "\tScale: {}", output.scale)?;
                writeln!(&mut writer, "\tTransform: {:?}", output.transform)?;

                // Modes
                writeln!(&mut writer, "\tModes:")?;
                for (mode_idx, mode) in output.modes.iter().enumerate() {
                    let mut mode_buffer = String::new();
                    mode_buffer.push_str(&format!(
                        "{}x{}@{}",
                        mode.dimensions.0, mode.dimensions.1, mode.refresh
                    ));
                    if mode.preferred {
                        mode_buffer.push_str(" preferred");
                    }
                    if Some(mode_idx) == output.active_mode_idx {
                        mode_buffer.push_str(" active");
                    }

                    writeln!(&mut writer, "\t\t{mode_buffer}")?;
                }
                writeln!(&mut writer, "---")?;
            }
        }
        fht_compositor_ipc::Response::Windows(windows) => {
            for (_, window) in windows {
                print_window(&mut writer, window)?;
                writeln!(&mut writer, "---")?;
            }
        }
        fht_compositor_ipc::Response::LayerShells(layer_shells) => {
            for (idx, layer_shell) in layer_shells.iter().enumerate() {
                writeln!(&mut writer, "Layer Shell #{idx}")?;
                writeln!(&mut writer, "\tOutput: {}", layer_shell.output)?;
                writeln!(&mut writer, "\tNamespace: {}", layer_shell.namespace)?;
                writeln!(&mut writer, "\tLayer: {:?}", layer_shell.layer)?;
                writeln!(
                    &mut writer,
                    "\tKeyboard Interactivity: {:?}",
                    layer_shell.keyboard_interactivity
                )?;
                writeln!(&mut writer, "---")?;
            }
        }
        fht_compositor_ipc::Response::Space(fht_compositor_ipc::Space {
            monitors,
            primary_idx,
            active_idx,
        }) => {
            writeln!(&mut writer, "Space")?;
            for (idx, (_, monitor)) in monitors.iter().enumerate() {
                writeln!(&mut writer, "\tMonitor #{idx}:")?;
                writeln!(&mut writer, "\t\tOutput: {}", monitor.output)?;
                writeln!(&mut writer, "\t\tPrimary: {}", idx == *primary_idx)?;
                writeln!(&mut writer, "\t\tActive: {}", idx == *active_idx)?;
                writeln!(&mut writer, "\t\tWorkspaces: {:#?}", monitor.workspaces)?;
                writeln!(&mut writer, "---")?;
            }
            //
        }

        fht_compositor_ipc::Response::Window(window) => match window.as_ref() {
            Some(window) => print_window(&mut writer, window)?,
            None => println!("No focused window"),
        },
        fht_compositor_ipc::Response::Workspace(workspace) => {
            let Some(workspace) = workspace else {
                writeln!(&mut writer, "No workspace with given ID!")?;
                return Ok(());
            };

            writeln!(&mut writer, "ID: {}", workspace.id)?;
            writeln!(&mut writer, "Windows: {:?}", workspace.windows)?;
            writeln!(&mut writer, "Master width factor: {}", workspace.mwfact)?;
            writeln!(
                &mut writer,
                "Number of master windows: {}",
                workspace.nmaster
            )?;
        }
        fht_compositor_ipc::Response::PickedLayerShell(result) => match result {
            PickLayerShellResult::Some(layer_shell) => {
                writeln!(&mut writer, "\tOutput: {}", layer_shell.output)?;
                writeln!(&mut writer, "\tNamespace: {}", layer_shell.namespace)?;
                writeln!(&mut writer, "\tLayer: {:?}", layer_shell.layer)?;
                writeln!(
                    &mut writer,
                    "\tKeyboard Interactivity: {:?}",
                    layer_shell.keyboard_interactivity
                )?;
            }
            PickLayerShellResult::None => writeln!(
                &mut writer,
                "User picked something that is not a layer-shell"
            )?,
            PickLayerShellResult::Cancelled => {
                writeln!(&mut writer, "Pick layer-shell request was cancelled")?
            }
        },
        fht_compositor_ipc::Response::PickedWindow(result) => match result {
            PickWindowResult::Some(window) => writeln!(&mut writer, "Picked window ID: {window}")?,
            PickWindowResult::None => {
                writeln!(&mut writer, "User picked something that is not a window")?
            }
            PickWindowResult::Cancelled => {
                writeln!(&mut writer, "Pick window request was cancelled")?
            }
        },
        fht_compositor_ipc::Response::CursorPosition { x, y } => {
            writeln!(&mut writer, "Cursor position: {x}, {y}")?;
        }
        fht_compositor_ipc::Response::Error(err) => anyhow::bail!(err.clone()),
        fht_compositor_ipc::Response::Noop => (),
    }

    Ok(())
}

fn print_window(
    writer: &mut impl std::io::Write,
    window: &fht_compositor_ipc::Window,
) -> std::io::Result<()> {
    writeln!(writer, "Window #{}", window.id)?;
    writeln!(writer, "\tTitle: {:?}", window.title)?;
    writeln!(writer, "\tApplication ID: {:?}", window.app_id)?;
    writeln!(writer, "\tCurrent workspace ID: {}", window.workspace_id)?;
    writeln!(writer, "\tSize: ({}, {})", window.size.0, window.size.1)?;
    writeln!(
        writer,
        "\tLocation: ({}, {})",
        window.location.0, window.location.1
    )?;
    writeln!(writer, "\tFullscreened: {}", window.fullscreened)?;
    writeln!(writer, "\tMaximized: {}", window.maximized)?;
    writeln!(writer, "\tTiled: {}", window.tiled)?;
    writeln!(writer, "\tActivated: {}", window.activated)?;
    writeln!(writer, "\tFocused: {}", window.focused)?;
    Ok(())
}
