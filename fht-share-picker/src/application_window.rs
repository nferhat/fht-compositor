use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::Application;

mod imp {
    use std::{cell::RefCell, sync::OnceLock};

    use adw::subclass::prelude::AdwApplicationWindowImpl;
    use glib::{object::ObjectExt, subclass::Signal, types::StaticType};
    use gtk::prelude::{ButtonExt, WidgetExt};

    use crate::selection_widget::SelectionWidget;

    use super::*;

    #[derive(Default, Debug, gtk::CompositeTemplate)]
    #[template(resource = "/fht/desktop/SharePicker/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub select_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub selection_widget: TemplateChild<SelectionWidget>,
        // The current selection held by the user.
        //
        // HACK: I hate the fact we are storing it as a json string, but I'm too lazy to represent
        // an enum with values with glib::value::Value\
        pub current_selection: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "SharePickerWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        // You must call `Widget`'s `init_template()` within `instance_init()`.
        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            self.selection_widget.set_window(&self.obj());
            self.selection_widget.connect_local(
                "selection-changed",
                false,
                glib::clone!(
                    #[weak(rename_to = imp)]
                    self,
                    #[upgrade_or]
                    None,
                    move |args| {
                        let json_string: Option<String> = args[1].get().unwrap();
                        imp.select_button.set_sensitive(json_string.is_some());
                        *imp.current_selection.borrow_mut() = json_string;
                        None
                    }
                ),
            );
            self.select_button.connect_clicked(glib::clone!(
                #[weak(rename_to = imp)]
                self,
                move |_| {
                    // SAFETY: We know that the select button is only sensitive if there's something selected.
                    let selection = imp.current_selection.borrow().clone().unwrap();
                    imp.obj()
                        .emit_by_name::<()>("source-selected", &[&selection]);
                }
            ));
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("source-selected")
                    .param_types([String::static_type()])
                    .build()]
            })
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {
        // Save window state on delete event
        fn close_request(&self) -> glib::Propagation {
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}

    #[gtk::template_callbacks]
    impl Window {
        #[template_callback]
        fn on_cancel_clicked(&self) {}
    }
}

glib::wrapper! {
pub struct Window(ObjectSubclass<imp::Window>)
    @extends adw::ApplicationWindow, gtk::Widget, gtk::Window, gtk::ApplicationWindow,
    @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
    gtk::Native, gtk::Root, gtk::ShortcutManager, gio::ActionMap, gio::ActionGroup;
}

impl Window {
    pub fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}
