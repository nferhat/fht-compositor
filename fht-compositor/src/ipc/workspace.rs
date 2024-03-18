use smithay::reexports::calloop;
use zbus::interface;

pub enum Request {
    ChangeMasterWidthFactor { delta: f32 },
    ChangeNmaster { delta: i32 },
    SelectNextLayout,
    SelectPreviousLayout,
    FocusNextWindow,
    FocusPreviousWindow,
}

pub struct Workspace {
    // Channels to communicate with the compositor.
    to_compositor: calloop::channel::Sender<Request>,
    // from_compositor: async_channel::Receiver<Response>,
    /// The windows stored in this workspace, their its ID to be exact.
    ///
    /// You can get more information about a window with the `fht.desktop.Compositor.Window`
    /// interface.
    pub windows: Vec<u64>,

    /// The focused window index.
    ///
    /// WARNING: In the workspace code it's actually a usize, but I don't think you will ever
    /// exceed 255 windows in a single workspace (if you are a sane person, that is.)
    pub focused_window_index: u8,

    /// The fullscreen window for this workspace, its ID to be exact.
    pub fullscreen: Option<u64>,

    /// The active layout name.
    pub active_layout: String,

    /// Whether this workspace is the focused one on its output.
    pub active: bool,
}

impl Workspace {
    pub fn new(active: bool, active_layout: String) -> (Self, calloop::channel::Channel<Request>) {
        let (to_compositor, from_ipc_channel) = calloop::channel::channel();

        (
            Self {
                to_compositor,
                windows: vec![],
                focused_window_index: 0,
                fullscreen: None,
                active_layout,
                active,
            },
            from_ipc_channel,
        )
    }
}

#[interface(name = "fht.desktop.Compositor.Workspace")]
impl Workspace {
    async fn change_master_width_factor(&self, delta: f32) {
        if let Err(err) = self
            .to_compositor
            .send(Request::ChangeMasterWidthFactor { delta })
        {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn change_nmaster(&self, delta: i32) {
        if let Err(err) = self.to_compositor.send(Request::ChangeNmaster { delta }) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn select_next_layout(&self) {
        if let Err(err) = self.to_compositor.send(Request::SelectNextLayout) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn select_previous_layout(&self) {
        if let Err(err) = self.to_compositor.send(Request::SelectPreviousLayout) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn focus_next_window(&self) {
        if let Err(err) = self.to_compositor.send(Request::FocusNextWindow) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    async fn focus_previous_window(&self) {
        if let Err(err) = self.to_compositor.send(Request::FocusPreviousWindow) {
            warn!(?err, "Failed to send IPC request to the compositor!");
        }
    }

    #[zbus(property)]
    async fn windows(&self) -> &[u64] {
        self.windows.as_slice()
    }

    #[zbus(property)]
    async fn focused_window(&self) -> u64 {
        self.fullscreen
            .or_else(|| {
                self.windows
                    .get(self.focused_window_index as usize)
                    .map(|v| *v)
            })
            .unwrap_or(0)
    }

    #[zbus(property)]
    async fn fullscreen(&self) -> Option<u64> {
        self.fullscreen
    }

    #[zbus(property)]
    async fn active_layout(&self) -> &str {
        &self.active_layout
    }

    #[zbus(property)]
    async fn active(&self) -> bool {
        self.active
    }
}
