//! fht-compositor screencast source picker.
//!
//! Look I get its not the best looking application in the world but it does the job
//!
//! I'm willing to rework the visuals when I implement a custom theme for iced
use std::str::FromStr;

use iced::{widget as w, Element};
mod theme;

#[macro_use]
extern crate tracing;

fn main() -> iced::Result {
    // Logging.
    // color_eyre for pretty panics
    color_eyre::install().unwrap();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::from_str(if cfg!(debug) || cfg!(debug_assertions) {
            "debug"
        } else {
            // We only care about the fatal stuff here
            "error"
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

    iced::program("Select screencast source", State::update, State::view)
        .settings(iced::Settings {
            window: iced::window::Settings {
                decorations: false,
                platform_specific: iced::window::settings::PlatformSpecific {
                    application_id: String::from("fht.desktop.ScreenCastSourcePicker"),
                },
                ..Default::default()
            },
            default_text_size: 13.0.into(),
            default_font: iced::Font {
                weight: iced::font::Weight::Normal,
                ..iced::Font::with_name("Zed Mono")
            },
            fonts: vec![include_bytes!("../res/zed-mono-extended.ttf")
                .as_slice()
                .into()],
            antialiasing: true,
            ..Default::default()
        })
        .theme(|_| iced::Theme::Dark)
        .run()
}

#[derive(Default)]
enum Pane {
    /// Show the user the possible source types.
    #[default]
    SourceType,
    /// Show the user the list of select outputs.
    Outputs(Vec<Output>),
}

impl Pane {
    fn view(&self) -> Element<'_, Message> {
        match self {
            Self::SourceType => w::column![
                simple_button("Select area".into(), Message::SelectArea),
                simple_button("Select output".into(), Message::SelectOutput),
            ]
            .spacing(5)
            .into(),
            Self::Outputs(outputs) => {
                let mut outputs_col = w::column![].spacing(10);
                for Output {
                    name,
                    location,
                    size,
                } in outputs
                {
                    let content =
                        format!("Output {name} located on {location:?} with size {size:?}");
                    let content =
                        simple_button(content, Message::SelectedOutput { name: name.clone() });
                    outputs_col = outputs_col.push(content);
                }

                outputs_col.into()
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Message {
    /// Go back to the [`SourceType`] pane
    GoBack,
    /// The user requested to select a specific area.
    SelectArea,
    /// The user requested to select an output.
    SelectOutput,
    /// The user select an output.
    SelectedOutput { name: String },
}

struct State {
    /// The active displayed pane.
    active_pane: Pane,
}

/// A representation of output data.
#[derive(Debug)]
struct Output {
    /// The name of the output, exposed by fht-compositor dbus api.
    name: String,
    /// The location of the output, exposed by fht-compositor dbus api.
    location: (i32, i32),
    /// The size of the output, exposed by fht-compositor dbus api.
    size: (i32, i32),
}

impl Default for State {
    fn default() -> Self {
        let initial_message = std::env::args()
            .skip(1)
            .next()
            .and_then(|arg| match arg.as_str() {
                "select_outputs" => Some(Message::SelectOutput),
                "select_area" => Some(Message::SelectArea),
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

impl State {
    fn update(&mut self, message: Message) -> iced::Command<Message> {
        match message {
            Message::GoBack => self.active_pane = Pane::SourceType,
            // TODO: Use ron to serialize the message data (instead of whathever sorcery we are
            // doing here that is prone to failure, but for now we are simple enough.)
            Message::SelectArea => {
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
            Message::SelectOutput => {
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
            Message::SelectedOutput { name } => {
                eprintln!("[select-output]/{name}");
                std::process::exit(0);
            }
        }

        iced::Command::none()
    }

    fn view(&self) -> iced::Element<Message> {
        let pane_content = self.active_pane.view();
        let pane_content = w::container(pane_content)
            .padding(10.0)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .align_y(iced::alignment::Vertical::Top)
            .style(|_| theme::container::surface());

        let top_row: iced::Element<Message> = match &self.active_pane {
            Pane::SourceType => w::row!["fht-compositor screencast source picker"]
                .align_items(iced::Alignment::Center)
                .height(50.0)
                .into(),
            Pane::Outputs(_) => {
                let text: iced::Element<Message> = w::container(
                    w::text("fht-compositor screencast source picker: select output")
                        .vertical_alignment(iced::alignment::Vertical::Center),
                )
                .align_x(iced::alignment::Horizontal::Left)
                .center_y()
                .into();

                let button = w::container(
                    w::button(w::text("Go back"))
                        .width(iced::Length::Shrink)
                        .height(iced::Length::Shrink)
                        .style(|_, _| theme::button::primary())
                        .on_press(Message::GoBack),
                )
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Center);

                w::row![text, w::horizontal_space(), button]
                    .align_items(iced::Alignment::Center)
                    .height(50.0)
                    .into()
            }
        };
        let top_row = w::container(top_row)
            .padding(5)
            .height(iced::Length::Fixed(40.0))
            .width(iced::Length::Fill)
            .align_y(iced::alignment::Vertical::Top)
            .style(|_| theme::container::default());

        w::container(w::column![top_row, pane_content]).into()
    }
}

fn simple_button<'a>(content: String, on_press: Message) -> iced::Element<'a, Message> {
    use iced::widget::{button, container, text};
    container(
        button(
            container(text(content))
                .center_x()
                .center_y(),
        )
        .width(iced::Length::Fill)
        .height(iced::Length::Shrink)
        .style(|_, _| theme::button::elevated())
        .on_press(on_press),
    )
    .into()
}
