use std::borrow::BorrowMut;

use glam::{Mat3, Vec2};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, Uniform};
use smithay::backend::renderer::glow::{GlowFrame, GlowRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

use super::shaders::Shaders;
use super::FhtRenderer;
#[cfg(feature = "udev-backend")]
use crate::backend::udev::{UdevFrame, UdevRenderError, UdevRenderer};

// NOTE: While no, we do not support fractional scaling in the compositor, to comply with Smithay's
// (Render)Element traits, the functions exposed here use f64 scales
#[derive(Debug)]
pub struct RoundedWindowElement<R: FhtRenderer> {
    element: WaylandSurfaceRenderElement<R>,
    corner_radius: f32,
    input_to_geo: Mat3,
    // where is the rounded rectangle that is going to contain everything.
    geo: Rectangle<i32, Logical>,
}

impl<R: FhtRenderer> RoundedWindowElement<R> {
    pub fn new(
        element: WaylandSurfaceRenderElement<R>,
        corner_radius: f32,
        geometry: Rectangle<i32, Logical>,
        scale: impl Into<Scale<f64>>,
    ) -> Self {
        let scale = scale.into();
        // Cool trick for subsurfaces geometry by niri. (I am sometimes too stupid todo stuff)
        // We transform the coordinates normalized in the shader to our global coordinates.
        let elem_geo = element.geometry(scale);

        let elem_geo_loc = Vec2::new(elem_geo.loc.x as f32, elem_geo.loc.y as f32);
        let elem_geo_size = Vec2::new(elem_geo.size.w as f32, elem_geo.size.h as f32);

        let geo = geometry.to_physical_precise_round(scale);
        let geo_loc = Vec2::new(geo.loc.x, geo.loc.y);
        let geo_size = Vec2::new(geo.size.w, geo.size.h);

        let transform = element.transform();
        // HACK: ??? for some reason flipped ones are fine.
        let transform = match transform {
            Transform::_90 => Transform::_270,
            Transform::_270 => Transform::_90,
            x => x,
        };
        let transform_matrix = Mat3::from_translation(Vec2::new(0.5, 0.5))
            * Mat3::from_cols_array(transform.matrix().as_ref())
            * Mat3::from_translation(-Vec2::new(0.5, 0.5));

        // FIXME: y_inverted
        let input_to_geo = transform_matrix
            * Mat3::from_scale(elem_geo_size / geo_size)
            * Mat3::from_translation((elem_geo_loc - geo_loc) / elem_geo_size);

        Self {
            element,
            corner_radius,
            geo: geometry,
            input_to_geo,
        }
    }

    pub fn will_clip(
        elem: &WaylandSurfaceRenderElement<R>,
        scale: impl Into<Scale<f64>>,
        geometry: Rectangle<i32, Logical>,
        corner_radius: f32,
    ) -> bool {
        let scale = scale.into();
        let elem_geo = elem.geometry(scale);
        let geo = geometry.to_physical_precise_round(scale);

        // In case our corner radius is 0.0, we just want to see if we can hold this
        // surface in the render geometry. (no rounded corners)
        if corner_radius == 0.0 {
            !geo.contains_rect(elem_geo)
        } else {
            // Now we have our rounded corners, so we gotta calc where our rounded corners will
            // be and see if our surface geometry will intersect it.
            //
            // To check that we intersect atleast one, we first remove our rounded corners from
            // our main render geometry, then, when we remove this clip area
            // from our surface geo, check if its empty.
            // If it is, all the surface is contained within the clip area.
            //
            // This is kinda like a `Rectangle::intersect_any` function.
            let corners = Self::rounded_corners_regions(corner_radius, geometry, scale);
            let geo = Rectangle::subtract_rects_many([geo], corners);
            !Rectangle::subtract_rects_many([elem_geo], geo).is_empty()
        }
    }

    pub fn rounded_corners_regions(
        corner_radius: f32,
        geo: Rectangle<i32, Logical>,
        scale: Scale<f64>,
    ) -> [Rectangle<i32, Physical>; 4] {
        // You can imagine this rounded corners like four rectangles pertruding inside the main
        // opaque regions.
        //
        // OOxxxxxxxOO Here the "OO"s are meant to represented non-opaque regions for our render
        // xxxxxxxxxxx element, so they should not be blacked out by the damage tracker.
        // xxxxxxxxxxx
        // OOxxxxxxxOO

        // Even if we round and get up a little more, its no big deal if the ORs are offset by one
        // pixel or two.
        let corner_radius = corner_radius.clamp(0.0, f32::INFINITY).round() as i32;
        let corner_radius_size: Size<_, _> = (corner_radius, corner_radius).into();

        [
            Rectangle::new(geo.loc, corner_radius_size).to_physical_precise_round(scale), /* top left */
            Rectangle::new(
                (geo.loc.x + geo.size.w - corner_radius, geo.loc.y).into(),
                corner_radius_size,
            )
            .to_physical_precise_round(scale), /* top right */
            Rectangle::new(
                (
                    geo.loc.x + geo.size.w - corner_radius,
                    geo.loc.y + geo.size.h - corner_radius,
                )
                    .into(),
                corner_radius_size,
            )
            .to_physical_precise_round(scale), /* bottom right */
            Rectangle::new(
                (geo.loc.x, geo.loc.y + geo.size.h - corner_radius).into(),
                corner_radius_size,
            )
            .to_physical_precise_round(scale), /* bottom left */
        ]
    }
}

impl<R: FhtRenderer> Element for RoundedWindowElement<R> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.element.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }

    fn transform(&self) -> Transform {
        Transform::Normal
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.element.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let regions = self.element.opaque_regions(scale);

        // Intersect with geometry, since we're clipping by it.
        let mut geo = self.geo.to_physical_precise_round(scale);
        geo.loc -= self.geometry(scale).loc;
        let regions = regions
            .into_iter()
            .filter_map(|rect| rect.intersection(geo));

        // We are not clipping anything.
        if self.corner_radius == 0.0 {
            return regions.collect();
        }

        let corners = Self::rounded_corners_regions(self.corner_radius, self.geo, scale);

        let elem_loc = self.geometry(scale).loc;
        let corners = corners.into_iter().map(|mut rect| {
            rect.loc -= elem_loc;
            rect
        });

        OpaqueRegions::from_slice(Rectangle::subtract_rects_many(regions, corners).as_slice())
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }
}

impl RenderElement<GlowRenderer> for RoundedWindowElement<GlowRenderer> {
    fn draw(
        &self,
        frame: &mut GlowFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if self.corner_radius == 0.0 {
            self.element.draw(frame, src, dst, damage, opaque_regions)
        } else {
            // Override texture shader with our uniforms
            let program = Shaders::get_from_frame(frame.borrow_mut())
                .rounded_window
                .clone();
            let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(frame);

            gles_frame.override_default_tex_program(
                program,
                vec![
                    Uniform::new("geo_size", (self.geo.size.w as f32, self.geo.size.h as f32)),
                    Uniform::new("corner_radius", self.corner_radius),
                    super::mat3_uniform("input_to_geo", self.input_to_geo),
                ],
            );

            let res = self.element.draw(frame, src, dst, damage, opaque_regions);

            // Never forget to reset since its not our responsibility to manage texture shaders.
            BorrowMut::<GlesFrame>::borrow_mut(frame).clear_tex_program_override();

            res
        }
    }

    fn underlying_storage(&self, renderer: &mut GlowRenderer) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}

#[cfg(feature = "udev-backend")]
impl<'a> RenderElement<UdevRenderer<'a>> for RoundedWindowElement<UdevRenderer<'a>> {
    fn draw(
        &self,
        frame: &mut UdevFrame<'a, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), UdevRenderError> {
        if self.corner_radius == 0.0 {
            self.element.draw(frame, src, dst, damage, opaque_regions)
        } else {
            let glow_frame = frame.as_mut();
            let program = Shaders::get_from_frame(glow_frame.borrow_mut())
                .rounded_window
                .clone();
            let gles_frame: &mut GlesFrame = BorrowMut::borrow_mut(glow_frame);

            gles_frame.override_default_tex_program(
                program,
                vec![
                    Uniform::new("geo_size", (self.geo.size.w as f32, self.geo.size.h as f32)),
                    Uniform::new("corner_radius", self.corner_radius),
                    super::mat3_uniform("input_to_geo", self.input_to_geo),
                ],
            );

            let res = self.element.draw(frame, src, dst, damage, opaque_regions);
            BorrowMut::<GlesFrame>::borrow_mut(frame.as_mut()).clear_tex_program_override();

            res
        }
    }

    fn underlying_storage(&self, renderer: &mut UdevRenderer<'a>) -> Option<UnderlyingStorage> {
        self.element.underlying_storage(renderer)
    }
}
