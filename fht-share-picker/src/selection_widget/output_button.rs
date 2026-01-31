use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::output_object::OutputObject;
use crate::utils::make_output_mode_string;

mod imp {
    use std::cell::RefCell;

    use glib::Binding;
    use gtk::CompositeTemplate;

    use super::*;

    #[derive(Default, Debug, CompositeTemplate)]
    #[template(resource = "/fht/desktop/SharePicker/ui/output-button.ui")]
    pub struct OutputButton {
        #[template_child]
        pub output_name: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub output_mode_text: TemplateChild<gtk::Inscription>,

        pub bindings: RefCell<Vec<Binding>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OutputButton {
        const NAME: &'static str = "OutputButton";
        type Type = super::OutputButton;
        type ParentType = gtk::ToggleButton;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for OutputButton {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for OutputButton {}
    impl ButtonImpl for OutputButton {}
    impl ToggleButtonImpl for OutputButton {}

    impl OutputButton {}
}

glib::wrapper! {
    pub struct OutputButton(ObjectSubclass<imp::OutputButton>)
        @extends gtk::ToggleButton, gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable,
        gtk::Actionable;
}

impl OutputButton {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_output(&self, output_obj: &OutputObject) {
        let imp = self.imp();
        let mut bindings = imp.bindings.borrow_mut();

        let name_binding = output_obj
            .bind_property("name", &*imp.output_name, "text")
            .sync_create()
            .build();
        bindings.push(name_binding);

        // TODO: Use binding for this? Though we don't live update outputs, this would still be
        // nice to have
        let mode_string = make_output_mode_string(output_obj);
        imp.output_mode_text.set_min_chars(mode_string.len() as u32);
        imp.output_mode_text.set_text(Some(&mode_string));
    }

    pub fn output_name(&self) -> String {
        let imp = self.imp();
        imp.output_name.text().unwrap().to_string()
    }
}
