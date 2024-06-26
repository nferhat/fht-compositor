mod buffer;
mod compositor;
mod data_control;
mod data_device;
mod dmabuf;
mod dnd;
#[cfg(feature = "udev_backend")]
mod drm_lease;
mod fractional_scale;
mod idle_inhibit;
mod input_method;
mod keyboard_shortcuts_inhibit;
mod layer_shell;
mod output;
mod pointer_constraints;
mod pointer_gestures;
mod presentation;
mod primary_selection;
mod relative_pointer;
mod screencopy;
mod seat;
mod security_context;
mod selection;
mod shm;
mod viewporter;
mod virtual_keyboard;
mod xdg_activation;
mod xdg_decoration;
mod xdg_shell;
