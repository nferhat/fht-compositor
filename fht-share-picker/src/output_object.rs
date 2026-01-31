//! Output object.
//!
//! Wrapper around [`Output`](fht_compositor_ipc::Output) to be used in GTK's ListModel

use glib::Object;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

mod imp {

    use std::cell::RefCell;

    use glib::Properties;

    use super::*;

    #[derive(Properties, Default)]
    #[properties(wrapper_type = super::OutputObject)]
    pub struct OutputObject {
        // FIXME: Use Rc<str> instead of String? Would be 100% better
        #[property(get, set, type = String)]
        name: RefCell<String>,
        #[property(get, set, type = u32)]
        size_w: RefCell<u32>,
        #[property(get, set, type = u32)]
        size_h: RefCell<u32>,
        #[property(get, set, type = i32)]
        position_x: RefCell<i32>,
        #[property(get, set, type = i32)]
        position_y: RefCell<i32>,
        #[property(get, set, type = f64)]
        refresh: RefCell<f64>,
    }

    // The central trait for subclassing a GObject
    #[glib::object_subclass]
    impl ObjectSubclass for OutputObject {
        const NAME: &'static str = "OutputInfo";
        type Type = super::OutputObject;
    }

    // Trait shared by all GObjects
    #[glib::derived_properties]
    impl ObjectImpl for OutputObject {}
}

glib::wrapper! {
    pub struct OutputObject(ObjectSubclass<imp::OutputObject>);
}

impl Default for OutputObject {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl OutputObject {
    pub fn new(name: String, size: (u32, u32), position: (i32, i32), refresh: f64) -> Self {
        Object::builder()
            .property("name", name)
            .property("size-w", size.0)
            .property("size-h", size.1)
            .property("position-x", position.0)
            .property("position-y", position.1)
            .property("refresh", refresh)
            .build()
    }

    pub fn rect(&self) -> gdk::Rectangle {
        gdk::Rectangle::new(
            self.position_x(),
            self.position_y(),
            self.size_w() as i32,
            self.size_h() as i32,
        )
    }
}
