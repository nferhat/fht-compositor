use std::str::FromStr;

use iced::{Application, Element};

#[macro_use]
extern crate tracing;

fn main() -> anyhow::Result<()> {
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
            error!("Make sure you have slurp in your $PATH!");
            anyhow::bail!(err.to_string());
        }
        Err(err) => {
            error!(?err, "Failed to execute slop!");
            anyhow::bail!(err.to_string());
        }
    }

    let initial_action = std::env::args()
        .skip(1)
        .next()
        .and_then(|arg| match arg.as_str() {
            "select_outputs" => Some(ScreenCastSourcePickerMessage::SelectOutput),
            "select_area" => Some(ScreenCastSourcePickerMessage::SelectArea),
            _ => None,
        });

    ScreenCastSourcePicker::run(iced::Settings {
        antialiasing: true,
        exit_on_close_request: true,
        initial_surface: iced::wayland::InitialSurface::XdgWindow(
            iced::wayland::actions::window::SctkWindowSettings {
                app_id: Some("fht.desktop.ScreenCastSourcePicker".into()),
                title: Some("Screen cast source picker.".into()),
                client_decorations: false,
                resizable: Some(2.0),
                size: (510, 300),
                transparent: false,
                ..Default::default()
            },
        ),
        flags: initial_action,
        ..Default::default()
    })
    .map_err(|err| anyhow::anyhow!(err))
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

impl iced::Application for ScreenCastSourcePicker {
    type Executor = iced::executor::Default;
    type Message = ScreenCastSourcePickerMessage;
    type Theme = iced::Theme;
    type Flags = Option<ScreenCastSourcePickerMessage>;

    fn new(
        initial_message: Option<ScreenCastSourcePickerMessage>,
    ) -> (Self, iced::Command<Self::Message>) {
        (
            Self {
                active_pane: Pane::default(),
            },
            match initial_message {
                Some(action) => iced::Command::perform(async {}, |_| action),
                None => iced::Command::none(),
            },
        )
    }

    fn theme(&self, _id: iced::window::Id) -> Self::Theme {
        iced::Theme::Dark
    }

    fn title(&self, _id: iced::window::Id) -> String {
        String::from("fht-compositor ScreenCast source picker")
    }

    fn update(&mut self, message: Self::Message) -> iced::Command<Self::Message> {
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
                eprintln!("{out}");
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
                eprintln!("{name}");
                std::process::exit(0);
            }
        }

        iced::Command::none()
    }

    fn view(
        &self,
        _id: iced::window::Id,
    ) -> iced::Element<'_, Self::Message, Self::Theme, iced::Renderer> {
        use iced::widget::{button, column, container, row, text};

        match &self.active_pane {
            Pane::SourceType => {
                let content = column![row![area_button(), output_button()].spacing(10),];
                container(content).padding(50).center_x().center_y().into()
            }
            Pane::Outputs(outputs) => {
                let mut outputs_row = row![].spacing(10);
                for Output {
                    name,
                    location,
                    size,
                } in outputs
                {
                    let content =
                        format!("Output {name} located on {location:?} with size {size:?}");
                    let text = container(
                        button(text(content))
                            .on_press(ScreenCastSourcePickerMessage::SelectedOutput(name.clone())),
                    )
                    .center_x()
                    .center_y()
                    .width(410)
                    .height(40);
                    outputs_row = outputs_row.push(text);
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

static SELECTION_ICON_DATA: &[u8; 749] = include_bytes!("../res/selection.svg");

fn area_button<'a>() -> Element<'a, ScreenCastSourcePickerMessage> {
    use iced::widget::svg::Handle;
    use iced::widget::{button, column, container, svg};

    let svg_handle = Handle::from_memory(SELECTION_ICON_DATA);
    let icon = svg(svg_handle).width(130).height(130);
    let icon = container(icon).center_x().center_y().width(200).height(160);

    let text = container("Select area")
        .center_x()
        .center_y()
        .width(200)
        .height(40);

    button(container(column![text, icon]).width(200).height(200))
        .on_press(ScreenCastSourcePickerMessage::SelectArea)
        .into()
}

static DISPLAY_ICON_DATA: &[u8; 571] = include_bytes!("../res/display.svg");

fn output_button<'a>() -> Element<'a, ScreenCastSourcePickerMessage> {
    use iced::widget::svg::Handle;
    use iced::widget::{button, column, container, svg};

    let svg_handle = Handle::from_memory(DISPLAY_ICON_DATA);
    let icon = svg(svg_handle).width(130).height(130);
    let icon = container(icon).center_x().center_y().width(200).height(160);

    let text = container("Select output")
        .center_x()
        .center_y()
        .width(200)
        .height(40);

    button(container(column![text, icon]).width(200).height(200))
        .on_press(ScreenCastSourcePickerMessage::SelectOutput)
        .into()
}
