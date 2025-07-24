//! Companion program for `fht-compositor` screen cast support
//!
//! This program is used with the XDG screencast portal provided by `fht-compositor` in order to
//! let the user select what he wants to screen cast.
//!
//! We support three types of sources: Outputs, Workspaces, and individual windows.
//!
//! The data is gathered using the [`fht_compositor_ipc`] crate.

// GTK/adwaita boilerplate
mod application;
mod application_window;

mod output_object;
mod window_object;

mod selection_widget;
mod utils;

use std::io::{Read, Write};

use gtk::prelude::ApplicationExtManual;
use gtk::{gio, glib};
use output_object::OutputObject;
use window_object::WindowObject;

/// The selected screencast source
///
/// This exact same struct gets deserialized by the compositor when we are done with selecting,
/// from the [Standard Output](std::io::Stdout).
#[derive(serde::Serialize, serde::Deserialize)]
pub enum ScreencastSource {
    Window { id: usize },
    Workspace { output: String, idx: usize },
    Output { name: String },
}

fn main() -> glib::ExitCode {
    let only_message = tracing_subscriber::fmt::format::debug_fn(|writer, field, value| {
        if field.name() == "message" {
            write!(writer, "{value:?}")
        } else {
            Ok(())
        }
    });

    tracing_subscriber::fmt()
        .compact()
        .with_target(false)
        .fmt_fields(only_message)
        .with_writer(std::io::stderr)
        .init();

    glib::set_application_name("fht-share-picker");
    glib::log_set_default_handler(glib::rust_log_handler);
    gio::resources_register_include!("fht.desktop.SharePicker.gresource").unwrap();

    let app = application::Application::new();
    app.run()
}

fn get_compositor_data(
) -> Result<(Vec<WindowObject>, Vec<OutputObject>), Box<dyn std::error::Error>> {
    use fht_compositor_ipc::{connect, Request, Response};
    let (_, mut socket) = connect()?;

    let mut req = serde_json::to_string(&Request::Outputs)?;
    req.push('\n');
    _ = socket.write_all(req.as_bytes())?;
    let mut res_buf = String::new();
    _ = socket.read_to_string(&mut res_buf);
    let outputs: Response = serde_json::from_str(&res_buf)?;
    let Response::Outputs(outputs) = outputs else {
        unreachable!()
    };
    let outputs = outputs
        .into_values()
        .map(|o| {
            let active_mode = &o.modes[o.active_mode_idx.unwrap()];
            OutputObject::new(o.name, o.size, o.position, active_mode.refresh)
        })
        .collect();

    let (_, mut socket) = connect()?;
    let mut req = serde_json::to_string(&Request::Windows)?;
    req.push('\n');
    _ = socket.write(req.as_bytes())?;
    let mut res_buf = String::new();
    _ = socket.read_to_string(&mut res_buf);
    let windows: Response = serde_json::from_str(&res_buf)?;
    let Response::Windows(windows) = windows else {
        unreachable!()
    };
    let windows = windows
        .into_iter()
        .map(|win| {
            WindowObject::new(
                win.id as u64,
                win.title.unwrap_or_default(),
                win.app_id.unwrap_or_default(),
            )
        })
        .collect();

    Ok((windows, outputs))
}
