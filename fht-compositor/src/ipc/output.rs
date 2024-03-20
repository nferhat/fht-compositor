use smithay::reexports::calloop;

use crate::utils::output::OutputExt;

pub struct Output {
    // Channels to communicate with the compositor.
    to_compositor: calloop::channel::Sender<Request>,
    /// The name the output.
    pub name: String,

    /// The location of the output in global coordinate space.
    pub location: (i32, i32),

    /// The size of the output, aka it's Mode size.
    pub size: (i32, i32),

    /// The refresh rate of the output.
    pub refresh_rate: f32,

    /// The make/brand of this output's display.
    pub make: String,

    /// The model of this output's display.
    pub model: String,

    /// The fractional scale of the output, advertised for protocols that support it.
    pub fractional_scale: f64,

    /// The integer scale of the output, advertised for the protocols that don't yet support
    /// fractional scale.
    pub integer_scale: i32,

    /// The active workspace index for this output.
    pub active_workspace_index: u8,
}

pub enum Request {
    SetActiveWorkspaceIndex { index: u8 },
}

impl Output {
    pub fn new(
        output: &smithay::output::Output,
    ) -> (Self, String, calloop::channel::Channel<Request>) {
        let name = output.name();
        let path = format!("/fht/desktop/Compositor/Output/{}", name.replace("-", "_"));

        let geometry = output.geometry();
        let physical_properties = output.physical_properties();
        let mode = output.current_mode().unwrap();
        let integer_scale = output.current_scale().integer_scale();
        let fractional_scale = output.current_scale().fractional_scale();
        // WARN: I assume this factory function gets called when the output is added ONLY.
        let active_idx = 0u8;

        let (to_compositor, from_ipc_channel) = calloop::channel::channel::<Request>();

        (
            Self {
                to_compositor,
                name,
                location: (geometry.loc.x, geometry.loc.y),
                size: (geometry.size.w, geometry.size.h),
                refresh_rate: mode.refresh as f32 / 1_000.0,
                make: physical_properties.make,
                model: physical_properties.model,
                fractional_scale,
                integer_scale,
                active_workspace_index: active_idx as u8,
            },
            path,
            from_ipc_channel,
        )
    }
}

#[zbus::interface(name = "fht.desktop.Compositor.Output")]
impl Output {
    #[zbus(property)]
    fn name(&self) -> &str {
        &self.name
    }

    #[zbus(property)]
    fn location(&self) -> (i32, i32) {
        self.location
    }

    #[zbus(property)]
    fn size(&self) -> (i32, i32) {
        self.size
    }

    #[zbus(property)]
    fn refresh_rate(&self) -> f32 {
        self.refresh_rate
    }

    #[zbus(property)]
    fn make(&self) -> &str {
        &self.make
    }

    #[zbus(property)]
    fn model(&self) -> &str {
        &self.model
    }

    #[zbus(property)]
    fn fractional_scale(&self) -> f64 {
        self.fractional_scale
    }

    #[zbus(property)]
    fn integer_scale(&self) -> i32 {
        self.integer_scale
    }

    #[zbus(property)]
    fn active_workspace_index(&self) -> u8 {
        self.active_workspace_index
    }

    #[zbus(property)]
    fn set_active_workspace_index(&self, index: u8) {
        if let Err(err) = self
            .to_compositor
            .send(Request::SetActiveWorkspaceIndex { index })
        {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }
}
