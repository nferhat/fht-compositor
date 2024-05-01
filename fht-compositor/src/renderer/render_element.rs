//! A meta render element enum storing everything element that we render.
//!
//! Each of these is in reality nested enums that all implement RenderElement for GlowRenderer and
//! UdevRenderer.
//!
//! TODO: Write custom render elements macro to save my life.

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesTexture};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Transform};

use super::{AsGlowFrame, AsGlowRenderer};
#[cfg(feature = "udev_backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};
use crate::shell::cursor::CursorRenderElement;
use crate::shell::window::FhtWindowRenderElement;
use crate::shell::workspaces::WorkspaceSetRenderElement;

pub enum FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    Cursor(CursorRenderElement<R>),
    Egui(TextureRenderElement<GlesTexture>),
    Wayland(WaylandSurfaceRenderElement<R>),
    WorkspaceSet(WorkspaceSetRenderElement<R>),
}

impl<R> From<WaylandSurfaceRenderElement<R>> for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn from(value: WaylandSurfaceRenderElement<R>) -> Self {
        Self::Wayland(value)
    }
}

impl<R> From<CursorRenderElement<R>> for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn from(value: CursorRenderElement<R>) -> Self {
        Self::Cursor(value)
    }
}

impl<R> Element for FhtRenderElement<R>
where
    R: Renderer + ImportAll + ImportMem,
    <R as Renderer>::TextureId: Clone + 'static,

    CursorRenderElement<R>: RenderElement<R>,
    FhtWindowRenderElement<R>: RenderElement<R>,
    WaylandSurfaceRenderElement<R>: RenderElement<R>,
    WorkspaceSetRenderElement<R>: RenderElement<R>,
{
    fn id(&self) -> &Id {
        match self {
            Self::Cursor(e) => e.id(),
            Self::Egui(e) => e.id(),
            Self::Wayland(e) => e.id(),
            Self::WorkspaceSet(e) => e.id(),
        }
    }

    fn current_commit(&self) -> CommitCounter {
        match self {
            Self::Cursor(e) => e.current_commit(),
            Self::Egui(e) => e.current_commit(),
            Self::Wayland(e) => e.current_commit(),
            Self::WorkspaceSet(e) => e.current_commit(),
        }
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        match self {
            Self::Cursor(e) => e.location(scale),
            Self::Egui(e) => e.location(scale),
            Self::Wayland(e) => e.location(scale),
            Self::WorkspaceSet(e) => e.location(scale),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Cursor(e) => e.src(),
            Self::Egui(e) => e.src(),
            Self::Wayland(e) => e.src(),
            Self::WorkspaceSet(e) => e.src(),
        }
    }

    fn transform(&self) -> Transform {
        match self {
            Self::Cursor(e) => e.transform(),
            Self::Egui(e) => e.transform(),
            Self::Wayland(e) => e.transform(),
            Self::WorkspaceSet(e) => e.transform(),
        }
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Cursor(e) => e.geometry(scale),
            Self::Egui(e) => e.geometry(scale),
            Self::Wayland(e) => e.geometry(scale),
            Self::WorkspaceSet(e) => e.geometry(scale),
        }
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            Self::Cursor(e) => e.damage_since(scale, commit),
            Self::Egui(e) => e.damage_since(scale, commit),
            Self::Wayland(e) => e.damage_since(scale, commit),
            Self::WorkspaceSet(e) => e.damage_since(scale, commit),
        }
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        match self {
            Self::Cursor(e) => e.opaque_regions(scale),
            Self::Egui(e) => e.opaque_regions(scale),
            Self::Wayland(e) => e.opaque_regions(scale),
            Self::WorkspaceSet(e) => e.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        match self {
            Self::Cursor(e) => e.alpha(),
            Self::Egui(e) => e.alpha(),
            Self::Wayland(e) => e.alpha(),
            Self::WorkspaceSet(e) => e.alpha(),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Cursor(e) => e.kind(),
            Self::Egui(e) => e.kind(),
            Self::Wayland(e) => e.kind(),
            Self::WorkspaceSet(e) => e.kind(),
        }
    }
}

impl RenderElement<GlowRenderer> for FhtRenderElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Cursor(e) => e.draw(frame, src, dst, damage),
            Self::Egui(e) => {
                <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
                    e, frame, src, dst, damage,
                )
            }
            Self::Wayland(e) => e.draw(frame, src, dst, damage),
            Self::WorkspaceSet(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage> {
        match self {
            Self::Cursor(e) => e.underlying_storage(renderer),
            Self::Egui(e) => e.underlying_storage(renderer),
            Self::Wayland(e) => e.underlying_storage(renderer),
            Self::WorkspaceSet(e) => e.underlying_storage(renderer),
        }
    }
}

#[cfg(feature = "udev_backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for FhtRenderElement<UdevRenderer<'a>> {
    fn draw<'frame>(
        &self,
        frame: &mut UdevFrame<'a, 'frame>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        match self {
            Self::Cursor(e) => e.draw(frame, src, dst, damage),
            Self::Egui(e) => {
                let frame = frame.glow_frame_mut();
                <TextureRenderElement<GlesTexture> as RenderElement<GlowRenderer>>::draw(
                    e, frame, src, dst, damage,
                )
                .map_err(|err| UdevRenderError::Render(err))
            }
            Self::Wayland(e) => e.draw(frame, src, dst, damage),
            Self::WorkspaceSet(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(&self, renderer: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage> {
        match self {
            Self::Cursor(e) => e.underlying_storage(renderer),
            Self::Egui(e) => e.underlying_storage(renderer.glow_renderer_mut()),
            Self::Wayland(e) => e.underlying_storage(renderer),
            Self::WorkspaceSet(e) => e.underlying_storage(renderer),
        }
    }
}
