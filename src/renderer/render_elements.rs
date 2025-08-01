// niri_render_elements => fht_render_elements
//
// From https://github.com/Yalter/Niri licensed under GPL-v3.0
// Thank you very much.
#[macro_export]
macro_rules! fht_render_elements {
    // The two callable variants: with <R> and without <R>. They include From impls because nested
    // repetitions ($type and $variant with + and $R with ?) don't work properly.
    ($name:ident<R> => { $($variant:ident = $type:ty),+ $(,)? }) => {
        $crate::fht_render_elements!(@impl $name () ($name<R>) => { $($variant = $type),+ });

        $(impl<R: $crate::renderer::FhtRenderer> From<$type> for $name<R> {
            fn from(x: $type) -> Self {
                Self::$variant(x)
            }
        })+
    };

    ($name:ident => { $($variant:ident = $type:ty),+ $(,)? }) => {
        $crate::fht_render_elements!(@impl $name ($name) () => { $($variant = $type),+ });

        $(impl From<$type> for $name {
            fn from(x: $type) -> Self {
                Self::$variant(x)
            }
        })+
    };

    // The internal variant that generates most of the code. $name_no_R and $name_R are necessary
    // for the impl RenderElement<SomeRenderer> for $name<SomeRenderer>: since $R does not appear
    // in this line, we cannot condition based on $R like elsewhere, so we condition on duplicate
    // names instead. Like this: $($name_R<SomeRenderer>)? $($name_no_R)? so only one is chosen.
    (@impl $name:ident ($($name_no_R:ident)?) ($($name_R:ident<$R:ident>)?) => { $($variant:ident = $type:ty),+ }) => {
        #[derive(Debug)]
        #[allow(clippy::large_enum_variant)]
        pub enum $name$(<$R: $crate::renderer::FhtRenderer>)? {
            $($variant($type)),+
        }

        impl$(<$R: $crate::renderer::FhtRenderer>)? smithay::backend::renderer::element::Element for $name$(<$R>)? {
            fn id(&self) -> &smithay::backend::renderer::element::Id {
                match self {
                    $($name::$variant(elem) => elem.id()),+
                }
            }

            fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
                match self {
                    $($name::$variant(elem) => elem.current_commit()),+
                }
            }

            fn geometry(&self, scale: smithay::utils::Scale<f64>) -> smithay::utils::Rectangle<i32, smithay::utils::Physical> {
                match self {
                    $($name::$variant(elem) => elem.geometry(scale)),+
                }
            }

            fn transform(&self) -> smithay::utils::Transform {
                match self {
                    $($name::$variant(elem) => elem.transform()),+
                }
            }

            fn src(&self) -> smithay::utils::Rectangle<f64, smithay::utils::Buffer> {
                match self {
                    $($name::$variant(elem) => elem.src()),+
                }
            }

            fn damage_since(
                &self,
                scale: smithay::utils::Scale<f64>,
                commit: Option<smithay::backend::renderer::utils::CommitCounter>,
            ) -> smithay::backend::renderer::utils::DamageSet<i32, smithay::utils::Physical> {
                match self {
                    $($name::$variant(elem) => elem.damage_since(scale, commit)),+
                }
            }

            fn opaque_regions(&self, scale: smithay::utils::Scale<f64>) -> smithay::backend::renderer::utils::OpaqueRegions<i32, smithay::utils::Physical> {
                match self {
                    $($name::$variant(elem) => elem.opaque_regions(scale)),+
                }
            }

            fn alpha(&self) -> f32 {
                match self {
                    $($name::$variant(elem) => elem.alpha()),+
                }
            }

            fn kind(&self) -> smithay::backend::renderer::element::Kind {
                match self {
                    $($name::$variant(elem) => elem.kind()),+
                }
            }
        }

        impl smithay::backend::renderer::element::RenderElement<smithay::backend::renderer::glow::GlowRenderer>
            for $($name_R<smithay::backend::renderer::glow::GlowRenderer>)? $($name_no_R)?
        {
            fn draw(
                &self,
                frame: &mut smithay::backend::renderer::glow::GlowFrame<'_, '_>,
                src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
                dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
                damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
                opaque_regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
            ) -> Result<(), smithay::backend::renderer::gles::GlesError> {
                match self {
                    $($name::$variant(elem) => {
                        smithay::backend::renderer::element::RenderElement::<smithay::backend::renderer::glow::GlowRenderer>::draw(elem, frame, src, dst, damage, opaque_regions)
                    })+
                }
            }

            fn underlying_storage(&self, renderer: &mut smithay::backend::renderer::glow::GlowRenderer) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
                match self {
                    $($name::$variant(elem) => elem.underlying_storage(renderer)),+
                }
            }
        }

        #[cfg(feature = "udev-backend")]
        impl<'render> smithay::backend::renderer::element::RenderElement<$crate::backend::udev::UdevRenderer<'render>>
            for $($name_R<$crate::backend::udev::UdevRenderer<'render>>)? $($name_no_R)?
        {
            fn draw(
                &self,
                frame: &mut $crate::backend::udev::UdevFrame<'render, '_, '_>,
                src: smithay::utils::Rectangle<f64, smithay::utils::Buffer>,
                dst: smithay::utils::Rectangle<i32, smithay::utils::Physical>,
                damage: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
                opaque_regions: &[smithay::utils::Rectangle<i32, smithay::utils::Physical>],
            ) -> Result<(), $crate::backend::udev::UdevRenderError> {
                match self {
                    $($name::$variant(elem) => {
                        smithay::backend::renderer::element::RenderElement::<$crate::backend::udev::UdevRenderer<'render>>::draw(elem, frame, src, dst, damage, opaque_regions)
                    })+
                }
            }

            fn underlying_storage(
                &self,
                renderer: &mut $crate::backend::udev::UdevRenderer<'render>,
            ) -> Option<smithay::backend::renderer::element::UnderlyingStorage> {
                match self {
                    $($name::$variant(elem) => elem.underlying_storage(renderer)),+
                }
            }
        }
    };
}
