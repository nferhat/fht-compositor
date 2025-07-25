use smithay::backend::renderer::element::RenderElementStates;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::Transform;

use crate::output::RedrawState;
use crate::state::Fht;
use crate::utils::get_monotonic_time;

pub struct HeadlessData {
    output: Output,
}

impl HeadlessData {
    pub fn new(fht: &mut Fht) -> Self {
        // Create a dummy output to initiate the workspace system that depends on one.
        let name = String::from("headless-0");
        let props = PhysicalProperties {
            make: String::from("fht-compositor"),
            model: String::from("headless-output"),
            size: (0, 0).into(),
            subpixel: Subpixel::None,
        };
        let output = Output::new(name, props);
        let mode = Mode {
            refresh: 60_000,
            size: (1920, 1080).into(),
        };
        output.add_mode(mode);
        output.set_preferred(mode);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(smithay::output::Scale::Integer(1)),
            Some((0, 0).into()),
        );

        fht.add_output(output.clone(), None, false);

        Self { output }
    }

    pub fn render(&mut self, fht: &mut Fht) -> anyhow::Result<bool> {
        // We are assured that there's only a single output.

        // This pretty much does the same thing as Winit but without:
        // 1. Creating render elements since there's nowhere to draw them
        // 2. Damage tracking whatsoever
        // 3. Buffer submitting
        // 1. Damage tracking
        let states = RenderElementStates::default();
        let mut feedbacks = fht.take_presentation_feedback(&self.output, &states);
        feedbacks.presented::<_, smithay::utils::Monotonic>(
            get_monotonic_time(),
            smithay::wayland::presentation::Refresh::Unknown,
            0,
            wp_presentation_feedback::Kind::empty(),
        );

        let output_state = fht.output_state.get_mut(&self.output).unwrap();
        output_state.current_frame_sequence = output_state.current_frame_sequence.wrapping_add(1);
        match std::mem::replace(&mut output_state.redraw_state, RedrawState::Idle) {
            RedrawState::Queued => (),
            _ => unreachable!(),
        }

        if output_state.animations_running {
            output_state.redraw_state = RedrawState::Queued;
        }

        // FIXME: Perhaps now always mark this as presented with damage?
        Ok(true)
    }
}
