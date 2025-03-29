# Important software

To get the best experience with the compositor, you should install additional services and tools to
get a more complete desktop session. Most importantly, you should have a terminal (of your choice
though the default configuration has [Alacritty](https://github.com/Alacritty/Alacritty)),
a text editor of your choice, and an app launcher.

Desktops environments like GNOME/KDE have all of these bundled together, `fht-compositor` does not
do that, so you are **very strongly** recommended to read this page.


## Must-have services

- **Sound**: You most likely want to have sound working on your session. Install
  [PipeWire](https://www.pipewire.org/) otherwise your desktop will be mute. PipeWire should
  be autostarted if you are under a systemd setup, otherwise, add to `autostart` section.<br>
  If you want to use the XDG screencast portal, PipeWire is required!

- **Notification daemon**: Many apps require one and might freeze (for example Discord) if no one
  is found. [mako](https://github.com/emersion/mako) is simple and works fine, otherwise, use
  whatever suits you.

- **Policy-kit daemon**: (abbreviated to polkit): Required to give system(root) access to regular
  applications in a safe and controlled manner. Refer to the
  [Arch Linux wiki page](https://wiki.archlinux.org/title/Polkit#Authentication_agents)
  on the topic and install the one you prefer.<br>
  If you are using the [Nix flake](../nix/flake.md), `polkit-gnome` has already been installed
  and should be autostarted with the session.

- **XDG desktop portal**: The compositor binary itself will start a session d-bus connection and
  expose the `ScreenCast` interfaces. However, other interfaces are **NOT** implemented, this is why
  you should fall back to
  [xdg-desktop-portal-gtk](https://github.com/flatpak/xdg-desktop-portal-gtk)

## Desktop Shell

Most desktop interface utilities like shells/panels use `wlr-layer-shell` under the hood to create
the surfaces to draw onto. `fht-compositor` implements said protocol so all the utilities/programs
you are used to will work fine!

- **Wallpaper**: [swww](https://github.com/LGFae/swww), [swaybg](https://github.com/swaywm/swaybg)
  or [wbg](https://codeberg.org/dnkl/wbg) will work fine.

- **App launcher**: [wofi](https://hg.sr.ht/~scoopta/wofi),
  [bemenu](https://github.com/Cloudef/bemenu), [fuzzel](https://codeberg.org/dnkl/fuzzel),
  [Anyrun](https://github.com/anyrun-org/anyrun), are all fine.

- **Desktop shell**:
  - [**Elkowar's Wacky Widgets**](https://github.com/elkowar/eww): provides you with a DSL to write
    GTK3 based panels, with built-in support for JSON, listeners, SCSS/SASS.
  - [**Astal**](https://github.com/aylur/astal): A framework to build desktop shells with GTK using
    Typescript or Lua (or any language that has GObject-Introspection).<br>
    Provides a lot of batteries to get you started (watchers for bluetooth, battery, notifications,
    etc.)
  - [**Quickshell**](https://git.outfoxxed.me/outfoxxed/quickshell): Qt-based building blocks for
    your desktop with QML.
  - [**Waybar**](https://github.com/Alexays/Waybar): Good'ol waybar, nothing fancy, but gets the
    job done fast and easy.

## Nice to have

While these are not required, the following tools are compatible with `fht-compositor` and provide
extra functionality that is *nice to have*.

- [**swayidle**](https://github.com/swaywm/swayidle): An idle management daemon, IE. run commands
  after idling for a while, for example turning off your screen after 10 minutes.
- [**cliphist**](https://github.com/sentriz/cliphist): A very simple clipboard manager for Wayland
  sessions.
- [**wl-clip-persist**](https://github.com/Linus789/wl-clip-persist): Make selections from an
  application persist even after closing it.
