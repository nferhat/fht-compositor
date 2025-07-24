mod layout;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};

use super::output_button::OutputButton;

mod imp {

    use super::*;

    #[derive(Default)]
    pub struct OutputGrid;

    #[glib::object_subclass]
    impl ObjectSubclass for OutputGrid {
        const NAME: &'static str = "OutputGrid";
        type Type = super::OutputGrid;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            // We make use of the custom layout manager
            klass.set_layout_manager_type::<layout::OutputGridLayout>();
            klass.set_css_name("geometry-container");
        }
    }

    impl ObjectImpl for OutputGrid {
        fn constructed(&self) {
            self.parent_constructed();
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for OutputGrid {}
    impl BoxImpl for OutputGrid {}
}

glib::wrapper! {
    pub struct OutputGrid(ObjectSubclass<imp::OutputGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl OutputGrid {
    pub fn add_output(&self, widget: &OutputButton, rect: gdk::Rectangle) {
        widget.set_parent(self);
        let layout_mgr = self.layout_manager().unwrap();
        let output_layout_mgr = layout_mgr
            .downcast_ref::<layout::OutputGridLayout>()
            .unwrap();
        output_layout_mgr.add_output(widget, rect);
    }

    pub fn deselect_all(&self) {
        let mut next_child = self.first_child();
        while let Some(child) = next_child {
            let button = child.downcast_ref::<OutputButton>().unwrap();
            button.set_active(false);
            next_child = child.next_sibling();
        }
    }
}
