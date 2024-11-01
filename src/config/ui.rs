use std::path::PathBuf;
use std::time::Duration;

use fht_animation::curve::Easing;
use fht_animation::{get_monotonic_time, Animation, AnimationCurve};
use smithay::backend::renderer::element::utils::RelocateRenderElement;
use smithay::reexports::rustix::path::Arg;
use smithay::utils::{Logical, Point, Size};

use crate::egui::{EguiElement, EguiRenderElement};
use crate::output::OutputExt;
use crate::renderer::FhtRenderer;

// A 800x640 rectangle should always suffice to display egui with room to spare.
// egui will do its geometry and layout magic in order to keep some space for shadows, so we should
// not have to worry about that here.
const WIDTH: i32 = 800;
const HEIGHT: i32 = 640;
// Animations n' stuff
const OUTPUT_PADDING: i32 = 4;
const SHOWN_DURATION: Duration = Duration::from_secs(10);
const SLIDE_DURATION: Duration = Duration::from_millis(450);
const SLIDE_CURVE: AnimationCurve = AnimationCurve::Simple(Easing::EaseInOutCubic);

/// A [`ConfigUi`].
///
/// It informs the user about the success of reloading the configuration, or any errors that might
/// have occured while doing so.
pub struct ConfigUi {
    state: State,
    egui: EguiElement,
}

crate::fht_render_elements! {
    ConfigUiRenderElement => {
        Egui = RelocateRenderElement<EguiRenderElement>,
    }
}

#[derive(Debug)]
pub enum Content {
    /// The configuration was successfully reloaded from `paths`.
    Reloaded { paths: Vec<PathBuf> },
    /// The configuration has encountered an error while reloading.
    ReloadError { error: fht_compositor_config::Error },
}

#[derive(Debug, Default)]
enum State {
    /// The [`ConfigUi`] is sliding in/out, from the top of the screen.
    Sliding {
        animation: Animation<f64>,
        content: Content,
        hiding: bool,
    },
    /// The [`ConfigUi`] is shown for `duration`.
    Shown {
        content: Content,
        started_at: Duration,
        last_tick: Duration,
    },
    /// The [`ConfigUi`] is hidden.
    #[default]
    Hidden,
}

impl ConfigUi {
    /// Create a new [`ConfigUi`] instance.
    pub fn new() -> Self {
        Self {
            state: State::Hidden,
            egui: EguiElement::new(Size::from((WIDTH, HEIGHT))),
        }
    }

    /// Show the [`ConfigUi`] with the following [`ConfigUiState`].
    pub fn show(&mut self, content: Content, animate: bool) {
        // HACK: To make egui forget the last used Area size by the previous content
        // Waiting for emilk to add a way to reset the pre-computed size
        self.egui.reset_ctx();
        if animate {
            self.state = match std::mem::take(&mut self.state) {
                State::Hidden => State::Sliding {
                    animation: Animation::new(0.0, 1.0, SLIDE_DURATION).with_curve(SLIDE_CURVE),
                    hiding: false,
                    content,
                },
                State::Sliding {
                    animation,
                    content: state,
                    ..
                } => State::Sliding {
                    animation: Animation::new(*animation.value(), 1.0, SLIDE_DURATION)
                        .with_curve(SLIDE_CURVE),
                    hiding: false,
                    content: state,
                },
                State::Shown {
                    started_at,
                    last_tick,
                    ..
                } => State::Shown {
                    content,
                    started_at,
                    last_tick,
                },
            }
        } else {
            let now = get_monotonic_time();
            self.state = State::Shown {
                content,
                started_at: now,
                last_tick: now,
            };
        }
    }

    /// Advance the animations for this [`ConfigUi`].
    pub fn advance_animations(
        &mut self,
        target_presentation_time: Duration,
        animate: bool,
    ) -> bool {
        let mut animations_ongoing = false;
        self.state = match std::mem::take(&mut self.state) {
            State::Sliding {
                mut animation,
                content,
                hiding,
            } => {
                animations_ongoing = true;
                animation.tick(target_presentation_time);
                if animation.is_finished() {
                    if hiding {
                        State::Hidden
                    } else {
                        State::Shown {
                            content,
                            started_at: target_presentation_time,
                            last_tick: target_presentation_time,
                        }
                    }
                } else {
                    State::Sliding {
                        animation,
                        content,
                        hiding,
                    }
                }
            }
            State::Shown {
                content,
                started_at,
                mut last_tick,
            } => {
                animations_ongoing = true;
                if last_tick - started_at >= SHOWN_DURATION {
                    if animate {
                        State::Sliding {
                            animation: Animation::new(1.0, 0.0, SLIDE_DURATION)
                                .with_curve(SLIDE_CURVE),
                            content,
                            hiding: true,
                        }
                    } else {
                        State::Hidden
                    }
                } else {
                    last_tick = target_presentation_time;
                    State::Shown {
                        content,
                        started_at,
                        last_tick,
                    }
                }
            }
            hidden => hidden,
        };

        animations_ongoing
    }

    /// Return whether this [`ConfigUi`] is hidden.
    pub fn hidden(&self) -> bool {
        matches!(self.state, State::Hidden)
    }

    /// Render this [`ConfigUi`].
    pub fn render(
        &mut self,
        renderer: &mut impl FhtRenderer,
        output: &smithay::output::Output,
        scale: f64,
    ) -> Option<ConfigUiRenderElement> {
        if matches!(self.state, State::Hidden) {
            return None;
        }

        // We first render the egui inside the desized texture, then, use the ctx information
        // in order to properly center the egui content on the output.
        let egui_element = self
            .egui
            .render(
                renderer.glow_renderer_mut(),
                scale.round() as i32, // FIXME: fractional scale
                1.0,
                Point::default(),
                |ctx| ui(ctx, &self.state),
            )
            .inspect_err(|err| warn!(?err, "Failed to render egui for config ui"))
            .ok()?;

        let used_size = self.egui.ctx().used_size();
        let output_size = output.geometry().size;

        let x = (f64::from(output_size.w) - f64::from(used_size.x)).max(0.0) / 2.0;
        let y = match &self.state {
            State::Sliding { animation, .. } => {
                let total_height = f64::from(used_size.y) + f64::from(OUTPUT_PADDING * 2);
                -f64::from(used_size.y) + *animation.value() * total_height
            }
            State::Shown { .. } => f64::from(OUTPUT_PADDING * 2),
            State::Hidden => unreachable!(),
        };

        let loc = Point::<_, Logical>::from((x, y)).to_i32_round::<i32>();
        let element = RelocateRenderElement::from_element(
            egui_element,
            loc.to_physical_precise_round(scale),
            smithay::backend::renderer::element::utils::Relocate::Absolute,
        );

        Some(element.into())
    }
}

fn ui(ctx: &egui::Context, state: &State) {
    egui::Area::new(egui::Id::NULL).show(ctx, |ui| {
        let content = match state {
            State::Sliding { content, .. } | State::Shown { content, .. } => content,
            State::Hidden => unreachable!(),
        };

        const SHADOW: egui::Shadow = egui::Shadow::NONE;
        const STROKE: egui::Stroke = egui::Stroke {
            width: 2.0,
            color: egui::Color32::from_gray(0x3c),
        };
        const INNER_MARGIN: f32 = 8.0;

        match content {
            Content::Reloaded { paths } => {
                egui::Frame::window(ui.style())
                    .inner_margin(INNER_MARGIN)
                    .stroke(STROKE)
                    .shadow(SHADOW)
                    .show(ui, |ui| {
                        if paths.len() == 1 {
                            let path = &paths[0];
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.label("Reloaded your configuration from ");
                                egui::Frame::canvas(ui.style())
                                    .inner_margin(0.5)
                                    .show(ui, |ui| {
                                        ui.monospace(
                                            path.as_str()
                                                .expect("config path should only be valid UTF-8"),
                                        )
                                    });
                            });
                        } else {
                            ui.label("Reloaded from the following files: ");
                            ui.indent("config-paths-reloaded", |ui| {
                                for path in paths {
                                    egui::Frame::canvas(ui.style()).inner_margin(0.5).show(
                                        ui,
                                        |ui| {
                                            ui.monospace(
                                                path.as_str().expect(
                                                    "config path should only be valid UTF-8",
                                                ),
                                            )
                                        },
                                    );
                                }
                            });
                        }
                    });
            }
            Content::ReloadError { error } => {
                egui::Frame::canvas(ui.style())
                    .inner_margin(INNER_MARGIN)
                    .stroke(STROKE)
                    .shadow(SHADOW)
                    .show(ui, |ui| ui.monospace(error.to_string().trim()));
            }
        }
    });
}
