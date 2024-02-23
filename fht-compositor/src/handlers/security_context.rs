use std::sync::Arc;

use smithay::delegate_security_context;
use smithay::wayland::security_context::{
    SecurityContext, SecurityContextHandler, SecurityContextListenerSource,
};

use crate::state::{ClientState, State};

impl SecurityContextHandler for State {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        self.fht
            .loop_handle
            .insert_source(source, move |client_stream, _, state| {
                let client_state = ClientState {
                    security_context: Some(context.clone()),
                    ..ClientState::default()
                };

                if let Err(err) = state
                    .fht
                    .display_handle
                    .insert_client(client_stream, Arc::new(client_state))
                {
                    warn!(?err, "Failed to add wayland client to display!");
                }
            })
            .expect("Failed to init Wayland security context source!");
    }
}

delegate_security_context!(State);
