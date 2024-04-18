use std::str::FromStr;

#[macro_use]
extern crate tracing;

fn main() -> iced::Result {
    // Logging.
    // color_eyre for pretty panics
    color_eyre::install().unwrap();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::from_str(if cfg!(debug) || cfg!(debug_assertions) {
            "trace"
        } else {
            "error,warn,fht_compositor=info"
        })
        .unwrap()
    });
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .without_time()
        .init();

    info!(
        version = std::env!("CARGO_PKG_VERSION"),
        git_hash = std::option_env!("GIT_HASH").unwrap_or("Unknown"),
        "Starting fht-share-picker."
    );

    // The only other dependency for this is slurp to select regions on the screen. It's not that
    // hard to implement but if I do it will be a pretty buggy and unstable mess.
    match std::process::Command::new("slurp").spawn() {
        Ok(mut child) => child.kill().unwrap(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            panic!("{}", format!("Make sure you have slurp in your $PATH!"));
        }
        Err(err) => {
            panic!("{}", format!("Failed to execute slop! {err}"));
        }
    }

    let size = iced::Size {
        width: 510.0,
        height: 300.0,
    };
    iced::program(
        "Select screencast source",
        ScreenCastSourcePicker::update,
        ScreenCastSourcePicker::view,
    )
    .settings(iced::Settings {
        window: iced::window::Settings {
            decorations: false,
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: String::from("fht.desktop.ScreenCastSourcePicker"),
            },
            size,
            ..Default::default()
        },
        ..Default::default()
    })
    .theme(|_| iced::Theme::Dark)
    .centered()
    .run()
}

struct ScreenCastSourcePicker {
    active_pane: Pane,
}

#[derive(Default)]
enum Pane {
    #[default]
    SourceType,
    Outputs(Vec<Output>),
}

#[derive(Debug)]
struct Output {
    name: String,
    location: (i32, i32),
    size: (i32, i32),
}

#[derive(Clone, Debug)]
pub enum ScreenCastSourcePickerMessage {
    SelectArea,
    SelectOutput,
    SelectedOutput(String),
}

impl Default for ScreenCastSourcePicker {
    fn default() -> Self {
        let initial_message = std::env::args()
            .skip(1)
            .next()
            .and_then(|arg| match arg.as_str() {
                "select_outputs" => Some(ScreenCastSourcePickerMessage::SelectOutput),
                "select_area" => Some(ScreenCastSourcePickerMessage::SelectArea),
                _ => None,
            });

        let mut ret = Self {
            active_pane: Pane::default(),
        };

        if let Some(initial_message) = initial_message {
            let _ = ret.update(initial_message);
        }

        ret
    }
}

impl ScreenCastSourcePicker {
    fn update(
        &mut self,
        message: ScreenCastSourcePickerMessage,
    ) -> iced::Command<ScreenCastSourcePickerMessage> {
        match message {
            ScreenCastSourcePickerMessage::SelectArea => {
                let mut command = std::process::Command::new("slurp");
                let output = match command.output() {
                    Ok(output) => output,
                    Err(err) => {
                        error!(?err, "Failed to get slurp output!");
                        std::process::exit(1);
                    }
                };

                let exit_code = output.status.code().unwrap();
                if exit_code != 0 {
                    error!(?exit_code, "slurp did not exit successfully!");
                    std::process::exit(1);
                }

                // We only need the coordinates of the selected rectangle.
                // The compositor part will parse the picker output and that's it really.
                let out = std::str::from_utf8(&output.stdout).expect("Invalid bytes in stdout!");

                let mut iter = out.split_whitespace();

                let mut coords = iter.next().unwrap().split(',');
                let x: i32 = coords
                    .next()
                    .expect("Malformated output from slurp!")
                    .to_string()
                    .trim()
                    .parse()
                    .unwrap();
                let y: i32 = coords
                    .next()
                    .expect("Malformated output from slurp!")
                    .to_string()
                    .trim()
                    .parse()
                    .unwrap();

                let mut size = iter.next().unwrap().split('x');
                let w: i32 = size
                    .next()
                    .expect("Malformated output from slurp!")
                    .to_string()
                    .trim()
                    .parse()
                    .unwrap();
                let h: i32 = size
                    .next()
                    .expect("Malformated output from slurp!")
                    .to_string()
                    .trim()
                    .parse()
                    .unwrap();

                eprintln!("[select-area]/({x}, {y}, {w}, {h})");
                std::process::exit(0);
            }
            ScreenCastSourcePickerMessage::SelectOutput => {
                // Again, the compositor will get the right coordinates from the passed in output.
                let connection = match zbus::blocking::Connection::session() {
                    Ok(conn) => conn,
                    Err(err) => {
                        error!(?err, "Failed to open dbus session connection!");
                        std::process::exit(1);
                    }
                };

                let proxy = zbus::blocking::Proxy::new(
                    &connection,
                    "fht.desktop.Compositor",
                    "/fht/desktop/Compositor",
                    "fht.desktop.Compositor.Ipc",
                )
                .unwrap();

                let outputs: Vec<Output> = proxy
                    .call::<_, _, Vec<zvariant::OwnedObjectPath>>("ListOutputs", &())
                    .unwrap()
                    .into_iter()
                    .map(|output_path| {
                        let proxy = zbus::blocking::Proxy::new(
                            &connection,
                            "fht.desktop.Compositor",
                            output_path,
                            "fht.desktop.Compositor.Output",
                        )
                        .unwrap();

                        Output {
                            name: proxy.get_property("Name").unwrap(),
                            location: proxy.get_property("Location").unwrap(),
                            size: proxy.get_property("Size").unwrap(),
                        }
                    })
                    .collect();
                self.active_pane = Pane::Outputs(outputs);
            }
            ScreenCastSourcePickerMessage::SelectedOutput(name) => {
                // The compositor part will parse the named output and that's it really.
                eprintln!("[select-output]/{name}");
                std::process::exit(0);
            }
        }

        iced::Command::none()
    }

    fn view(&self) -> iced::Element<ScreenCastSourcePickerMessage> {
        use iced::widget::{column, container};

        match &self.active_pane {
            Pane::SourceType => {
                let content = column![
                    create_button(
                        "Select area".into(),
                        ScreenCastSourcePickerMessage::SelectArea
                    ),
                    create_button(
                        "Select output".into(),
                        ScreenCastSourcePickerMessage::SelectOutput
                    ),
                ]
                .spacing(10);

                container(content).padding(50).center_x().center_y().into()
            }
            Pane::Outputs(outputs) => {
                let mut outputs_row = column![].spacing(10);
                for Output {
                    name,
                    location,
                    size,
                } in outputs
                {
                    let content =
                        format!("Output {name} located on {location:?} with size {size:?}");
                    let content = create_button(
                        content,
                        ScreenCastSourcePickerMessage::SelectedOutput(name.clone()),
                    );
                    outputs_row = outputs_row.push(content);
                }

                container(outputs_row)
                    .padding(50)
                    .center_x()
                    .center_y()
                    .into()
            }
        }
    }
}

fn create_button<'a>(
    content: String,
    on_press: ScreenCastSourcePickerMessage,
) -> iced::Element<'a, ScreenCastSourcePickerMessage> {
    use iced::widget::{button, container, text};
    container(
        button(container(text(content)).center_x().center_y())
            .width(410)
            .height(40)
            .on_press(on_press),
    )
    .into()
}
