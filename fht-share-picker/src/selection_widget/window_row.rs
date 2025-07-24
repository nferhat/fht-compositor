use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::window_object::WindowObject;

mod imp {
    use std::cell::RefCell;

    use adw::subclass::prelude::{ActionRowImpl, PreferencesRowImpl};
    use glib::{Binding, Properties};

    use super::*;

    #[derive(Default, Debug, Properties)]
    #[properties(wrapper_type = super::WindowRow)]
    pub struct WindowRow {
        #[property(get, set, type = u64)]
        pub identifier: RefCell<u64>,
        pub bindings: RefCell<Vec<Binding>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WindowRow {
        const NAME: &'static str = "WindowRow";
        type Type = super::WindowRow;
        type ParentType = adw::ActionRow;
    }

    #[glib::derived_properties]
    impl ObjectImpl for WindowRow {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for WindowRow {}
    impl ListBoxRowImpl for WindowRow {}
    impl PreferencesRowImpl for WindowRow {}
    impl ActionRowImpl for WindowRow {}
}

glib::wrapper! {
    pub struct WindowRow(ObjectSubclass<imp::WindowRow>)
        @extends adw::ActionRow, gtk::ListBoxRow, adw::PreferencesRow, gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl WindowRow {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn bind_window(&self, window: &WindowObject) {
        let imp = self.imp();
        let mut bindings = imp.bindings.borrow_mut();

        let title_binding = window
            .bind_property("title", self, "title")
            .bidirectional()
            .sync_create()
            .build();
        bindings.push(title_binding);

        let app_id_binding = window
            .bind_property("app-id", self, "subtitle")
            .bidirectional()
            .sync_create()
            .build();
        bindings.push(app_id_binding);

        let identifier_binding = window
            .bind_property("identifier", self, "identifier")
            .bidirectional()
            .sync_create()
            .build();
        bindings.push(identifier_binding);
    }
}
