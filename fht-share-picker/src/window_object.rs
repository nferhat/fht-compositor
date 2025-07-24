//! Window object.
//!
//! Wrapper around [`Window`](fht_compositor_ipc::Window) to be used in GTK's ListModel

use glib::Object;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::WindowObject)]
    pub struct WindowObject {
        #[property(get, set, type = u64)]
        identifier: RefCell<u64>,
        #[property(get, set, type = String)]
        title: RefCell<String>,
        #[property(get, set, type = String)]
        app_id: RefCell<String>,
    }

    // The central trait for subclassing a GObject
    #[glib::object_subclass]
    impl ObjectSubclass for WindowObject {
        const NAME: &'static str = "WindowInfo";
        type Type = super::WindowObject;
    }

    // Trait shared by all GObjects
    #[glib::derived_properties]
    impl ObjectImpl for WindowObject {}
}

glib::wrapper! {
    pub struct WindowObject(ObjectSubclass<imp::WindowObject>);
}

impl WindowObject {
    pub fn new<Title, AppId>(identifier: u64, title: Title, app_id: AppId) -> Self
    where
        Title: AsRef<str>,
        AppId: AsRef<str>,
    {
        Object::builder()
            .property("identifier", identifier)
            .property("title", title.as_ref())
            .property("app-id", app_id.as_ref())
            .build()
    }
}
