use std::collections::HashMap;

use smithay::output::{self, Mode, Output};

use crate::delegate_output_management;
use crate::protocols::output_management::{
    OutputConfiguration, OutputManagementHandler, OutputManagementManagerState,
};
use crate::state::State;

impl OutputManagementHandler for State {
    fn output_management_manager_state(&mut self) -> &mut OutputManagementManagerState {
        &mut self.fht.output_management_manager_state
    }

    fn apply_configuration(&mut self, config: HashMap<Output, OutputConfiguration>) -> bool {
        // We filter by the outputs we know.
        let known_outputs = self.fht.space.outputs().cloned().collect::<Vec<_>>();
        let mut any_changed = false;

        for (output, config) in config
            .into_iter()
            .filter(|(output, _)| known_outputs.contains(output))
        {
            let output_name = output.name();
            debug!("Applying wlr-output-configuration for {output_name}");

            // FIXME: Handle output powered state
            let OutputConfiguration::Enabled {
                mode,
                position,
                transform,
                scale,
                adaptive_sync: _, // FIXME: Handle VRR
            } = config
            else {
                continue;
            };

            let changed =
                mode.is_some() || position.is_some() || transform.is_some() || scale.is_some();
            if !changed {
                continue;
            }

            if let Some(mode) = mode.map(|(size, refresh)| Mode {
                size,
                refresh: refresh.map(|v| v.get() as i32).unwrap_or(60000),
            }) {
                // First try to switch in the backend
                if let Err(err) = self.backend.set_output_mode(&mut self.fht, &output, mode) {
                    error!(
                        ?err,
                        "Failed to apply wlr-output-configuration mode for {output_name}"
                    );
                    return false;
                }
            }

            output.change_current_state(
                None,
                transform,
                scale.map(|scale| output::Scale::Integer(scale.round() as i32)),
                position,
            );

            if changed {
                self.fht.output_resized(&output);
                any_changed = true;
            }
        }

        true
    }

    fn test_configuration(&mut self, _config: HashMap<Output, OutputConfiguration>) -> bool {
        // FIXME: Actually test the configuration
        true
    }
}

delegate_output_management!(State);
