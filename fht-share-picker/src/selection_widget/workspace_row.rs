use adw::prelude::{ExpanderRowExt, PreferencesRowExt};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::output_object::OutputObject;
use crate::utils::make_output_mode_string;

mod imp {
    use std::sync::OnceLock;

    use glib::subclass::Signal;
    use gtk::CompositeTemplate;

    use super::*;
    use crate::utils::forall_siblings;

    #[derive(Default, Debug, CompositeTemplate)]
    #[template(resource = "/fht/desktop/SharePicker/ui/workspace-row.ui")]
    pub struct WorkspaceRow {
        #[template_child]
        pub index_buttons: TemplateChild<gtk::Box>,
        #[template_child]
        pub expander_row: TemplateChild<adw::ExpanderRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WorkspaceRow {
        const NAME: &'static str = "WorkspaceRow";
        type Type = super::WorkspaceRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for WorkspaceRow {
        fn constructed(&self) {
            self.parent_constructed();

            let mut index = 0;
            let mut next_sibling = self.index_buttons.first_child();
            while let Some(widget) = next_sibling {
                let toggle_button = widget.downcast_ref::<gtk::ToggleButton>().unwrap();
                toggle_button.set_label(&format!("{}", index + 1));
                toggle_button.add_css_class("circular");
                toggle_button.connect_clicked(glib::clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |button| {
                        if button.is_active() {
                            imp.obj().emit_by_name::<()>(
                                "workspace-selected",
                                &[&index as &dyn glib::value::ToValue],
                            );

                            forall_siblings(button, |sibling| {
                                let button = sibling.downcast_ref::<gtk::ToggleButton>().unwrap();
                                button.set_active(false);
                            });
                        }
                    },
                ));

                index += 1;
                next_sibling = widget.next_sibling();
            }
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("workspace-selected")
                    .param_types([i32::static_type()])
                    .build()]
            })
        }
    }

    impl WidgetImpl for WorkspaceRow {}
    impl BoxImpl for WorkspaceRow {}

    impl WorkspaceRow {}
}

glib::wrapper! {
    pub struct WorkspaceRow(ObjectSubclass<imp::WorkspaceRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl WorkspaceRow {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_output(&self, output_obj: &OutputObject) {
        let imp = self.imp();
        output_obj
            .bind_property("name", &*imp.expander_row, "title")
            .sync_create()
            .build();
        // TODO: Use binding for this? Though we don't live update outputs, this would still be
        // nice to have
        let mode_string = make_output_mode_string(output_obj);
        imp.expander_row.set_subtitle(&mode_string);
    }

    pub fn output_name(&self) -> String {
        let imp = self.imp();
        imp.expander_row.title().to_string()
    }

    pub fn set_expanded(&self, expanded: bool) {
        let imp = self.imp();
        imp.expander_row.set_expanded(expanded);
    }

    pub fn expanded(&self) -> bool {
        let imp = self.imp();
        imp.expander_row.is_expanded()
    }

    pub fn deselect_all_buttons(&self) {
        let imp = self.imp();
        let mut next_sibling = imp.index_buttons.first_child();
        while let Some(widget) = next_sibling {
            let toggle_button = widget.downcast_ref::<gtk::ToggleButton>().unwrap();
            toggle_button.set_active(false);
            next_sibling = widget.next_sibling();
        }
    }

    pub fn connect_workspace_selected<F: Fn(&Self, usize) + 'static>(&self, f: F) {
        self.connect_local("workspace-selected", false, move |args| {
            let this: &WorkspaceRow = args[0].get().unwrap();
            let index: i32 = args[1].get().unwrap();
            f(this, index as usize);

            None
        });
    }
}
