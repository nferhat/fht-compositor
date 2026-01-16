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

use std::io::{BufRead as _, BufReader, Write};
use std::os::unix::net::UnixStream;

pub use fht_compositor_ipc::ScreencastSource;
use fht_compositor_ipc::{Request, Response};
use gtk::prelude::ApplicationExtManual;
use gtk::{gio, glib};
use output_object::OutputObject;
use window_object::WindowObject;

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

fn write_req(
    stream: &mut UnixStream,
    req: Request,
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = serde_json::to_string(&req)?;
    req.push('\n'); // it is required to append a newline.
    stream.write_all(req.as_bytes()).unwrap();

    let mut reader = BufReader::new(stream);
    let mut res_buf = String::new();
    let size = reader.read_line(&mut res_buf)?;
    assert_eq!(res_buf.len(), size);

    let res = serde_json::de::from_str(&res_buf)?;
    Ok(res)
}

fn get_compositor_data(
) -> Result<(Vec<WindowObject>, Vec<OutputObject>), Box<dyn std::error::Error>> {
    let (_, mut socket) = fht_compositor_ipc::connect()?;

    let Response::Outputs(outputs) = write_req(&mut socket, Request::Outputs)? else {
        unreachable!()
    };
    let outputs = outputs
        .into_values()
        .map(|o| {
            let active_mode = &o.modes[o.active_mode_idx.unwrap()];
            OutputObject::new(o.name, o.size, o.position, active_mode.refresh)
        })
        .collect();

    let Response::Windows(windows) = write_req(&mut socket, Request::Windows)? else {
        unreachable!()
    };
    let windows = windows
        .into_iter()
        .map(|(id, win)| {
            WindowObject::new(
                id as u64,
                win.title.unwrap_or_default(),
                win.app_id.unwrap_or_default(),
            )
        })
        .collect();

    Ok((windows, outputs))
}
