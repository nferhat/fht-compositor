use glib::object::IsA;
use gtk::prelude::WidgetExt;

use crate::output_object::OutputObject;

pub fn forall_siblings<W: IsA<gtk::Widget>, F: Fn(&gtk::Widget)>(w: &W, f: F) {
    let mut next_sibling = w.next_sibling();
    while let Some(child) = next_sibling {
        f(&child);
        next_sibling = child.next_sibling();
    }

    let mut prev_sibling = w.prev_sibling();
    while let Some(child) = prev_sibling {
        f(&child);
        prev_sibling = child.prev_sibling();
    }
}

pub fn make_output_mode_string(obj: &OutputObject) -> String {
    format!("{}x{}@{}", obj.size_w(), obj.size_h(), obj.refresh())
}
