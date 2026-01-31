use gtk::prelude::LayoutManagerExt;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use crate::selection_widget::output_button::OutputButton;

mod imp {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gtk::prelude::WidgetExt;

    pub use super::*;

    pub struct OutputGridLayout {
        // Rectangles are tuples, with values in order (x, y, w, h)
        // TODO: I don't like the fact I am keeping strong references to the children
        pub output_geometries: RefCell<HashMap<OutputButton, gdk::Rectangle>>,
        pub total_area: RefCell<gdk::Rectangle>,
    }

    impl Default for OutputGridLayout {
        fn default() -> Self {
            Self {
                output_geometries: RefCell::default(),
                total_area: RefCell::new(gdk::Rectangle::new(0, 0, 0, 0)),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OutputGridLayout {
        const NAME: &'static str = "OutputGridLayout";
        type Type = super::OutputGridLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for OutputGridLayout {
        fn dispose(&self) {
            self.output_geometries.borrow_mut().clear();
            self.obj().layout_changed();
        }
    }

    impl LayoutManagerImpl for OutputGridLayout {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            _: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let horizontal = orientation == gtk::Orientation::Horizontal;
            let area = self.total_area.borrow();
            let (width, height) = (area.width(), area.height());

            let (main_size, secondary_size) = if horizontal {
                (width, height)
            } else {
                (height, width)
            };

            if main_size < 0 || secondary_size < 0 {
                return (0, 0, -1, -1);
            }

            let mut minimum @ mut natural = if for_size > 0 && secondary_size > 0 {
                for_size as f32 / secondary_size as f32
            } else {
                0f32
            };

            self.measure_scale(&mut minimum, &mut natural);

            (
                (minimum * main_size as f32).ceil() as i32,
                (natural * main_size as f32).ceil() as i32,
                -1,
                -1,
            )
        }

        fn allocate(&self, _: &gtk::Widget, width: i32, height: i32, _: i32) {
            if self.output_geometries.borrow().is_empty() {
                return;
            }

            let area = self.total_area.borrow();
            let scale = f32::min(
                width as f32 / area.width() as f32,
                height as f32 / area.height() as f32,
            );
            let scale = (scale * 10.).round() / 10.;
            let translate_x = (width as f32 - scale * area.width() as f32) / 2.;
            let translate_y = (height as f32 - scale * area.height() as f32) / 2.;

            let (area_x, area_y) = (area.x(), area.y());
            for (child, rect) in &*self.output_geometries.borrow() {
                if !child.should_layout() {
                    continue;
                }

                #[allow(unused_assignments)]
                let mut x1 @ mut x2 @ mut y1 @ mut y2 = 0f32;
                x1 = scale * (rect.x() - area_x) as f32 + translate_x;
                y1 = scale * (rect.y() - area_y) as f32 + translate_y;
                x2 = x1 + scale * rect.width() as f32;
                y2 = y1 + scale * rect.height() as f32;

                child.size_allocate(
                    &gtk::Allocation::new(
                        x1.ceil() as i32,
                        y1.ceil() as i32,
                        (x2 - x1).ceil() as i32,
                        (y2 - y1).ceil() as i32,
                    ),
                    -1,
                );
            }
        }
    }

    impl OutputGridLayout {
        pub fn measure_scale(&self, minimum: &mut f32, natural: &mut f32) {
            for (child, rect) in &*self.output_geometries.borrow() {
                if !child.should_layout() {
                    continue;
                }

                let (w, h) = (rect.width(), rect.height());
                let (child_minimum, child_natural, _, _) =
                    child.measure(gtk::Orientation::Horizontal, -1);
                *minimum = f32::max(*minimum, child_minimum as f32 / w as f32);
                *natural = f32::max(*natural, child_natural as f32 / w as f32);

                let (child_minimum, child_natural, _, _) =
                    child.measure(gtk::Orientation::Vertical, -1);
                *minimum = f32::max(*minimum, child_minimum as f32 / h as f32);
                *natural = f32::max(*natural, child_natural as f32 / h as f32);
            }

            *minimum = (*minimum * 10.).round() / 10.;
            *natural = (*natural * 10.).round() / 10.;
        }

        pub fn update_bounds(&self) {
            let total_area = self.output_geometries.borrow().values().fold(
                gdk::Rectangle::new(0, 0, 0, 0),
                |acc, rect| {
                    gdk::Rectangle::new(
                        acc.x().max(rect.x()),
                        acc.y().max(rect.y()),
                        acc.width().max(rect.width()),
                        acc.height().max(rect.height()),
                    )
                },
            );

            *self.total_area.borrow_mut() = total_area;
        }
    }
}

glib::wrapper! {
    pub struct OutputGridLayout(ObjectSubclass<imp::OutputGridLayout>)
        @extends gtk::LayoutManager;
}

impl OutputGridLayout {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn add_output(&self, widget: &OutputButton, rect: gdk::Rectangle) {
        let imp = self.imp();
        if !imp.output_geometries.borrow().contains_key(widget) {
            imp.output_geometries
                .borrow_mut()
                .insert(widget.clone(), rect);
        }

        imp.update_bounds();
        self.layout_changed();
    }
}
