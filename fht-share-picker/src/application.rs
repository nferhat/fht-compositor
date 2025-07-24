use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::application_window::Window;

mod imp {
    use std::cell::OnceCell;
    use std::io::Write;

    use adw::subclass::prelude::AdwApplicationImpl;
    use gio::prelude::ApplicationExt;
    use glib::object::ObjectExt;
    use glib::WeakRef;
    use gtk::prelude::GtkWindowExt;

    use super::*;

    #[derive(Debug, Default)]
    pub struct Application {
        pub window: OnceCell<WeakRef<Window>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "Application";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();

            if let Some(window) = self.window.get() {
                let window = window.upgrade().unwrap();
                window.present();

                return;
            }

            // This is just, yeah.
            // To bind globals that I need.
            let window = Window::new(&app);
            window.connect_local(
                "source-selected",
                false,
                glib::clone!(
                    #[weak(rename_to = imp)]
                    self,
                    #[upgrade_or]
                    None,
                    move |args| {
                        let json_content: String = args[1].get().unwrap();
                        std::io::stdout().write(json_content.as_bytes()).unwrap();
                        imp.obj().quit();
                        None
                    }
                ),
            );

            // Doing disgusting stuff in order to get access to a wl_display*;
            //
            // WHY? Simply because I want to bind globals myself in order to get information about
            // the running environment (I need foreign-toplevel-list and wl_output globals directly)
            self.window
                .set(ObjectExt::downgrade(&window))
                .expect("Window already set.");
            app.main_window().present();
        }

        fn startup(&self) {
            self.parent_startup();
            let provider = gtk::CssProvider::new();
            provider.load_from_resource("/fht/desktop/SharePicker/output-grid.css");
            // Add the provider to the default screen
            gtk::style_context_add_provider_for_display(
                &gdk::Display::default().expect("Could not connect to a display."),
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends gtk::Application, adw::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Application {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", "fht.desktop.SharePicker")
            .property("resource-base-path", "/fht/desktop/SharePicker/")
            .build()
    }

    fn main_window(&self) -> Window {
        self.imp().window.get().unwrap().upgrade().unwrap()
    }
}
