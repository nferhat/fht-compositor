pub mod hyprland_global_shortcuts_v1 {
    use smithay::reexports::wayland_server;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::backend as wayland_backend;
        wayland_scanner::generate_interfaces!("res/protocols/hyprland-global-shortcuts-v1.xml");
    }

    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("res/protocols/hyprland-global-shortcuts-v1.xml");
}
