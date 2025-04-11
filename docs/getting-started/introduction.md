# Introduction

`fht-compositor` is a dynamic tiling Wayland compositor based on the [Smithay](https://github.com/smithay)
compositor library. It has a layout model inspired by popular X11 window managers such as [DWM](https://dwm.suckless.org),
[AwesomeWM](https://awesomewm.org) and [XMonad](https://xmonad.org). Each connected output has 9 workspaces, each
workspace managing a bunch of windows.

Windows in the workspace are arranged using a *dynamic layout* in order to maximized the used area of your screen
in two stacks: The master and slave stack. Different parameters can be adjusted at runtime to adapt the different
dynamic layouts to your needs.

![Preview image](/assets/preview.png)

---

If this is your first time with the compositor, please head to the [install guide](./installing) to take
the [guided tour](/getting-started/guided-tour.md) in order to get up and running with the compositor!

> [!WARNING]
>
> `fht-compositor` is a bare-bones compositor, it does not include a bar, notifications, and other nice-to-have
> components that a full desktop environment like GNOME or KDE will provide. You are expected to tinker and work
> your way through error messages and configuration files!
>
> ---
>
> The compositor is still not *mature* yet, any feedback and reports would be greatly appreciated. If you have
> something not working right, you can file an [new issue](https://github.com/nferhat/fht-compositor/issues/new)
> or get in touch through the [matrix channel](https://matrix.to/#/#fht-compositor:matrix.org)!
