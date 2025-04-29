use std::io::{Read, Write as _};

use crate::cli;

/// Make a single request to the running `fht-compositor` IPC server.
///
/// It uses the IPC socket specified by the `FHTC_SOCKET_PATH` environment variable.
pub fn make_request(request: cli::Request, json: bool) -> anyhow::Result<()> {
    let request = match request {
        cli::Request::Version => fht_compositor_ipc::Request::Version,
        cli::Request::Outputs => fht_compositor_ipc::Request::Outputs,
        cli::Request::Windows => fht_compositor_ipc::Request::Windows,
        cli::Request::Space => fht_compositor_ipc::Request::Space,
        cli::Request::Action { action } => fht_compositor_ipc::Request::Action(action),
    };

    // This is just a re-implementation of fht-compositor-ipc/test_client with cleaner error
    // handling for error logging yada-yada
    let (_, mut stream) = fht_compositor_ipc::connect()?;
    stream.set_nonblocking(false)?;

    let mut req = serde_json::to_string(&request).unwrap();
    req.push('\n'); // it is required to append a newline.
    let size = stream.write(req.as_bytes()).unwrap();
    anyhow::ensure!(req.len() == size);

    let mut res_buf = String::new();
    let size = stream.read_to_string(&mut res_buf).unwrap();
    anyhow::ensure!(res_buf.len() == size);

    let response: Result<fht_compositor_ipc::Response, String> =
        serde_json::de::from_str(&res_buf)?;
    let response = match response {
        Ok(res) => res,
        Err(err) => anyhow::bail!("IPC error: {err}"),
    };

    if json {
        let json_buffer = match response {
            fht_compositor_ipc::Response::Version(version) => serde_json::to_string(&version),
            fht_compositor_ipc::Response::Outputs(hash_map) => serde_json::to_string(&hash_map),
            fht_compositor_ipc::Response::Windows(windows) => serde_json::to_string(&windows),
            fht_compositor_ipc::Response::Space(space) => serde_json::to_string(&space),
            fht_compositor_ipc::Response::Error(err) => {
                anyhow::bail!("Failed to handle IPC request: {err:?}")
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
        fht_compositor_ipc::Response::Version(version) => println!("{version}"),
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
            for window in windows {
                writeln!(&mut writer, "Window #{}", window.id)?;
                writeln!(&mut writer, "\tTitle: {:?}", window.title)?;
                writeln!(&mut writer, "\tApplication ID: {:?}", window.app_id)?;
                writeln!(&mut writer, "\tCurrent output: {}", window.output)?;
                writeln!(
                    &mut writer,
                    "\tCurrent workspace index: {}",
                    window.workspace_idx
                )?;
                writeln!(
                    &mut writer,
                    "\tCurrent workspace ID: {}",
                    window.workspace_id
                )?;
                writeln!(
                    &mut writer,
                    "\tSize: ({}, {})",
                    window.size.0, window.size.1
                )?;
                writeln!(
                    &mut writer,
                    "\tLocation: ({}, {})",
                    window.location.0, window.location.1
                )?;
                writeln!(&mut writer, "\tFullscreened: {}", window.fullscreened)?;
                writeln!(&mut writer, "\tMaximized: {}", window.maximized)?;
                writeln!(&mut writer, "\tTiled: {}", window.tiled)?;
                writeln!(&mut writer, "\tActivated: {}", window.activated)?;
                writeln!(&mut writer, "\tFocused: {}", window.focused)?;
                writeln!(&mut writer, "---")?;
            }
        }
        fht_compositor_ipc::Response::Space(fht_compositor_ipc::Space {
            monitors,
            primary_idx,
            active_idx,
        }) => {
            writeln!(&mut writer, "Space")?;
            for (idx, monitor) in monitors.iter().enumerate() {
                writeln!(&mut writer, "\tMonitor #{idx}:")?;
                writeln!(&mut writer, "\t\tOutput: {}", monitor.output)?;
                writeln!(&mut writer, "\t\tPrimary: {}", idx == *primary_idx)?;
                writeln!(&mut writer, "\t\tActive: {}", idx == *active_idx)?;
                writeln!(&mut writer, "\t\t---")?;
                for (workspace_idx, workspace) in monitor.workspaces.iter().enumerate() {
                    writeln!(&mut writer, "\t\tWorkspace #{workspace_idx}:")?;
                    writeln!(&mut writer, "\t\t\tID: {}", workspace.id)?;
                    writeln!(&mut writer, "\t\t\tWindows: {:?}", workspace.windows)?;
                    writeln!(
                        &mut writer,
                        "\t\t\tMaster width factor: {}",
                        workspace.mwfact
                    )?;
                    writeln!(
                        &mut writer,
                        "\t\t\tNumber of master windows: {}",
                        workspace.nmaster
                    )?;
                    writeln!(&mut writer, "\t\t---")?;
                }
            }
            //
        }
        fht_compositor_ipc::Response::Error(err) => anyhow::bail!(err.clone()),
        fht_compositor_ipc::Response::Noop => (),
    }

    Ok(())
}
