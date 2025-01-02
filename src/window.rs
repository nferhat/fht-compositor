use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use owning_ref::MutexGuardRef;
use smithay::backend::renderer::element;
use smithay::backend::renderer::element::surface::{
    render_elements_from_surface_tree, WaylandSurfaceRenderElement,
};
// use smithay::desktop::Window;
use smithay::desktop::utils::{
    bbox_from_surface_tree, output_update, send_dmabuf_feedback_surface_tree,
    send_frames_surface_tree, take_presentation_feedback_surface_tree, under_from_surface_tree,
    with_surfaces_surface_tree, OutputPresentationFeedback,
};
use smithay::desktop::{PopupManager, WindowSurfaceType};
use smithay::output::{Output, WeakOutput};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Serial, Size};
use smithay::wayland::compositor::{send_surface_state, with_states, HookId, SurfaceData};
use smithay::wayland::dmabuf::DmabufFeedback;
use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{SurfaceCachedState, ToplevelSurface, XdgToplevelSurfaceData};

use crate::renderer::FhtRenderer;
use crate::state::ResolvedWindowRules;

#[derive(Debug, Clone)]
pub struct Window {
    inner: Arc<WindowInner>,
}

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

impl WaylandFocus for Window {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        Some(Cow::Borrowed(self.inner.toplevel.wl_surface()))
    }
}

impl IsAlive for Window {
    fn alive(&self) -> bool {
        self.inner.toplevel.alive()
    }
}

static WINDOW_IDS: AtomicUsize = AtomicUsize::new(0);
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct WindowId(usize);
impl WindowId {
    pub fn unique() -> Self {
        Self(WINDOW_IDS.fetch_add(1, Ordering::SeqCst))
    }
}

#[derive(Debug)]
struct WindowInner {
    id: WindowId,
    toplevel: ToplevelSurface,
    data: Mutex<WindowData>,
}

// NOTE: This type is public just for the sake of getting the rules out of the window
// It is not meant to be accessed by the rest of the compositor logic, but owning_ref requires that
// the type is public, soo, can't do much about that :/
#[derive(Debug)]
pub struct WindowData {
    bbox: Rectangle<i32, Logical>,
    entered_outputs: HashMap<WeakOutput, Rectangle<i32, Logical>>,
    offscreen_element_id: Option<element::Id>,
    pre_commit_hook_id: Option<HookId>,
    // Rules need to be re-resolved when window state change
    rules: ResolvedWindowRules,
    need_to_resolve_rules: bool,
    foreign_toplevel_handle: Option<ForeignToplevelHandle>,
}

impl Window {
    pub fn new(toplevel: ToplevelSurface) -> Self {
        Self {
            inner: Arc::new(WindowInner {
                id: WindowId::unique(),
                toplevel,
                data: Mutex::new(WindowData {
                    bbox: Rectangle::default(),
                    entered_outputs: HashMap::new(),
                    offscreen_element_id: None,
                    pre_commit_hook_id: None,
                    rules: ResolvedWindowRules::default(),
                    need_to_resolve_rules: false,
                    foreign_toplevel_handle: None,
                }),
            }),
        }
    }

    pub fn downgrade(&self) -> WeakWindow {
        WeakWindow {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn id(&self) -> WindowId {
        self.inner.id
    }

    pub fn toplevel(&self) -> &ToplevelSurface {
        &self.inner.toplevel
    }

    pub fn send_pending_configure(&self) -> Option<Serial> {
        self.inner.toplevel.send_pending_configure()
    }

    pub fn send_configure(&self) -> Serial {
        self.toplevel().send_configure()
    }

    pub fn set_rules(&self, rules: ResolvedWindowRules) {
        let mut guard = self.inner.data.lock().unwrap();
        guard.need_to_resolve_rules = false;
        guard.rules = rules;
    }

    pub fn rules(&self) -> MutexGuardRef<WindowData, ResolvedWindowRules> {
        // Mutex madness, and interior mutability, I hate you :star_struck:
        let guard = self.inner.data.lock().unwrap();
        MutexGuardRef::new(guard).map(|data| &data.rules)
    }

    fn set_need_to_resolve_rules(&self) {
        let mut guard = self.inner.data.lock().unwrap();
        guard.need_to_resolve_rules = true;
    }

    pub fn need_to_resolve_rules(&self) -> bool {
        self.inner.data.lock().unwrap().need_to_resolve_rules
    }

    /// Set the foreign toplevel handle of this toplevel.
    ///
    /// NOTE: It is up to **you** to ensure that this handle is unique.
    pub fn set_foreign_toplevel_handle(&self, handle: ForeignToplevelHandle) {
        let mut guard = self.inner.data.lock().unwrap();
        // NOTE: Maybe using Weak<...> would be better here?
        guard.foreign_toplevel_handle = Some(handle);
    }

    /// Get a reference to the foreign toplevel handle of this toplevel.
    pub fn foreign_toplevel_handle(&self) -> Option<ForeignToplevelHandle> {
        let guard = self.inner.data.lock().unwrap();
        guard.foreign_toplevel_handle.clone()
    }

    /// Take the foreign toplevel handle of this toplevel.
    pub fn take_foreign_toplevel_handle(&self) -> Option<ForeignToplevelHandle> {
        let mut guard = self.inner.data.lock().unwrap();
        guard.foreign_toplevel_handle.take()
    }

    pub fn request_size(&self, new_size: Size<i32, Logical>) {
        self.toplevel().with_pending_state(|state| {
            state.size = Some(new_size);
        });
    }

    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        self.inner.data.lock().unwrap().bbox
    }

    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        if let Some(surface) = self.wl_surface() {
            for (popup, location) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                let offset = self.render_offset() + location - popup.geometry().loc;
                bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, offset));
            }
        }

        bounding_box
    }

    pub fn size(&self) -> Size<i32, Logical> {
        let bbox = self.bbox();
        if let Some(surface) = self.wl_surface() {
            // It's the set geometry clamped to the bounding box with the full bounding box as the
            // fallback.
            with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
                    .and_then(|geo| geo.intersection(bbox))
                    .map(|geo| geo.size)
            })
            .unwrap_or(bbox.size)
        } else {
            bbox.size
        }
    }

    /// Get the window visual's geometry start, relative to its buffer.
    /// This might be used for CSD, for example
    pub fn render_offset(&self) -> Point<i32, Logical> {
        let bbox = self.bbox();
        if let Some(surface) = self.wl_surface() {
            // It's the set geometry clamped to the bounding box with the full bounding box as the
            // fallback.
            with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
                    .and_then(|geo| geo.intersection(bbox))
                    .map(|geo| geo.loc)
            })
            .unwrap_or(bbox.loc)
        } else {
            bbox.loc
        }
    }

    pub fn request_fullscreen(&self, fullscreen: bool) {
        self.set_need_to_resolve_rules();
        self.toplevel().with_pending_state(|state| {
            if fullscreen {
                state.states.set(State::Fullscreen)
            } else {
                state.states.unset(State::Fullscreen)
            }
        });
    }

    pub fn fullscreen(&self) -> bool {
        self.toplevel()
            .with_pending_state(|state| state.states.contains(State::Fullscreen))
    }

    pub fn request_maximized(&self, maximize: bool) {
        self.set_need_to_resolve_rules();
        self.toplevel().with_pending_state(|state| {
            if maximize {
                state.states.set(State::Maximized)
            } else {
                state.states.unset(State::Maximized)
            }
        });
    }

    pub fn maximized(&self) -> bool {
        self.toplevel()
            .with_pending_state(|state| state.states.contains(State::Maximized))
    }

    pub fn request_bounds(&self, bounds: Option<Size<i32, Logical>>) {
        self.set_need_to_resolve_rules();
        self.toplevel()
            .with_pending_state(|state| state.bounds = bounds);
    }

    pub fn request_activated(&self, activated: bool) {
        self.set_need_to_resolve_rules();
        self.toplevel().with_pending_state(|state| {
            if activated {
                state.states.set(State::Activated)
            } else {
                state.states.unset(State::Activated)
            }
        });
    }

    // NOTE: Tiled implementation can vastly different by the client, since we have 4 possible
    // states to toggle on for tiling, and a client can check for any of these to determine whether
    // they are tiled.
    //
    // Another issue is that the clients also use the **tiled** property to check for CSD (bruh)

    pub fn request_tiled(&self, tiled: bool) {
        self.set_need_to_resolve_rules();
        self.toplevel().with_pending_state(|state| {
            if tiled {
                state.states.set(State::TiledLeft);
                state.states.set(State::TiledRight);
                state.states.set(State::TiledTop);
                state.states.set(State::TiledBottom);
            } else {
                state.states.unset(State::TiledLeft);
                state.states.unset(State::TiledRight);
                state.states.unset(State::TiledTop);
                state.states.unset(State::TiledBottom);
            }
        });
    }

    pub fn request_resizing(&self, resizing: bool) {
        self.toplevel().with_pending_state(|state| {
            if resizing {
                state.states.set(State::Resizing);
            } else {
                state.states.unset(State::Resizing);
            }
        })
    }

    pub fn tiled(&self) -> bool {
        self.toplevel().with_pending_state(|state| {
            state.states.contains(State::TiledLeft)
                || state.states.contains(State::TiledRight)
                || state.states.contains(State::TiledTop)
                || state.states.contains(State::TiledBottom)
        })
    }

    pub fn title(&self) -> Option<String> {
        with_states(self.wl_surface().as_deref()?, |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.title.clone()
        })
    }

    pub fn app_id(&self) -> Option<String> {
        with_states(self.wl_surface().as_deref()?, |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            data.app_id.clone()
        })
    }

    pub fn set_offscreen_element_id(&self, id: Option<element::Id>) {
        let mut guard = self.inner.data.lock().unwrap();
        guard.offscreen_element_id = id
    }

    pub fn offscreen_element_id(&self) -> Option<element::Id> {
        let guard = self.inner.data.lock().unwrap();
        guard.offscreen_element_id.clone()
    }

    pub fn set_pre_commit_hook_id(&self, id: HookId) {
        let mut guard = self.inner.data.lock().unwrap();
        guard.pre_commit_hook_id = Some(id);
    }

    pub fn take_pre_commit_hook_id(&self) -> Option<HookId> {
        let mut guard = self.inner.data.lock().unwrap();
        guard.pre_commit_hook_id.take()
    }

    pub fn enter_output(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        let mut guard = self.inner.data.lock().unwrap();
        guard.entered_outputs.insert(output.downgrade(), overlap);
        guard.entered_outputs.retain(|k, _| k.is_alive());
    }

    pub fn configure_for_output(&self, output: &Output) {
        let transform = output.current_transform();
        let scale = output.current_scale();
        self.with_surfaces(|surface, data| {
            send_surface_state(surface, data, scale.integer_scale(), transform);
        });

        self.with_surfaces(|_, data| {
            with_fractional_scale(data, |fractional_scale_state| {
                fractional_scale_state.set_preferred_scale(scale.fractional_scale());
            })
        });
    }

    pub fn leave_output(&self, output: &Output) {
        let mut guard = self.inner.data.lock().unwrap();
        let _ = guard.entered_outputs.remove(&output.downgrade());
        guard.entered_outputs.retain(|k, _| k.is_alive());
    }

    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
    {
        let time = time.into();
        if let Some(surface) = self.wl_surface() {
            send_frames_surface_tree(&surface, output, time, throttle, primary_scan_out_output);
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
            }
        }
    }

    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
        F: Fn(&WlSurface, &SurfaceData) -> &'a DmabufFeedback + Copy,
    {
        if let Some(surface) = self.wl_surface() {
            send_dmabuf_feedback_surface_tree(
                &surface,
                output,
                primary_scan_out_output,
                select_dmabuf_feedback,
            );
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                send_dmabuf_feedback_surface_tree(
                    surface,
                    output,
                    primary_scan_out_output,
                    select_dmabuf_feedback,
                );
            }
        }
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &SurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        if let Some(surface) = self.wl_surface() {
            take_presentation_feedback_surface_tree(
                &surface,
                output_feedback,
                primary_scan_out_output,
                presentation_feedback_flags,
            );
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                take_presentation_feedback_surface_tree(
                    surface,
                    output_feedback,
                    primary_scan_out_output,
                    presentation_feedback_flags,
                );
            }
        }
    }

    pub fn with_surfaces<F>(&self, mut processor: F)
    where
        F: FnMut(&WlSurface, &SurfaceData),
    {
        if let Some(surface) = self.wl_surface() {
            with_surfaces_surface_tree(&surface, &mut processor);
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                with_surfaces_surface_tree(surface, &mut processor);
            }
        }
    }

    pub fn on_commit(&self) {
        if let Some(surface) = self.wl_surface() {
            self.inner.data.lock().unwrap().bbox = bbox_from_surface_tree(&surface, (0, 0));
        }
    }

    pub fn refresh(&self) {
        let guard = self.inner.data.lock().unwrap();
        if let Some(surface) = self.wl_surface() {
            for (weak, overlap) in guard.entered_outputs.iter() {
                if let Some(output) = weak.upgrade() {
                    output_update(&output, Some(*overlap), &surface);
                    for (popup, location) in PopupManager::popups_for_surface(&surface) {
                        let mut overlap = *overlap;
                        overlap.loc -= location;
                        output_update(&output, Some(overlap), popup.wl_surface());
                    }
                }
            }
        }
    }

    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        let point = point.into();
        if let Some(surface) = self.wl_surface() {
            if surface_type.contains(WindowSurfaceType::POPUP) {
                for (popup, location) in PopupManager::popups_for_surface(&surface) {
                    let offset = self.render_offset() + location - popup.geometry().loc;
                    if let Some(result) =
                        under_from_surface_tree(popup.wl_surface(), point, offset, surface_type)
                    {
                        return Some(result);
                    }
                }
            }

            if surface_type.contains(WindowSurfaceType::TOPLEVEL) {
                return under_from_surface_tree(&surface, point, (0, 0), surface_type);
            }
        }

        None
    }

    pub fn render_toplevel_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        mut location: Point<i32, Physical>,
        scale: impl Into<Scale<f64>>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>> {
        let scale = scale.into();
        let Some(surface) = self.wl_surface() else {
            return vec![];
        };

        location -= self.render_offset().to_physical_precise_round(scale);
        render_elements_from_surface_tree(
            renderer,
            &surface,
            location,
            scale,
            alpha,
            element::Kind::Unspecified,
        )
    }

    pub fn render_popup_elements<R: FhtRenderer>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: impl Into<Scale<f64>>,
        alpha: f32,
    ) -> Vec<WaylandSurfaceRenderElement<R>> {
        let Some(surface) = self.wl_surface() else {
            return vec![];
        };
        let scale = scale.into();
        PopupManager::popups_for_surface(&surface)
            .flat_map(|(popup, popup_offset)| {
                let offset = (popup_offset - popup.geometry().loc).to_physical_precise_round(scale);

                render_elements_from_surface_tree(
                    renderer,
                    popup.wl_surface(),
                    location + offset,
                    scale,
                    alpha,
                    element::Kind::Unspecified,
                )
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct WeakWindow {
    inner: std::sync::Weak<WindowInner>,
}

impl PartialEq for WeakWindow {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        std::sync::Weak::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WeakWindow {}

impl WeakWindow {
    pub fn upgrade(&self) -> Option<Window> {
        self.inner.upgrade().map(|inner| Window { inner })
    }
}
