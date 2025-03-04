# Introduction

`fht-compositor` is a dynamic tiling Wayland compositor with a layout model inspired by the DWM,
AwesomeWM and XMonad window managers. Each connected output has 9 workspaces assigned to them,
with each of them having a number of windows.

Windows in the workspaces are managed using a dynamic layout in order to take advantage of the full
size of your output, and thus maximizing used screen real estate.

If this is your first time with the compositor, please head to [the install guide](./getting-started/install.md)
and then proceed to read the [guided tour](./getting-started/guided-tour.md) to get started with the compositor.

> ⚠️ **Warning**: `fht-compositor` is a bare-bones compositor, it does not include a bar,
> notifications, and other niceties of a full desktop. You are expected to be able to tinker and
> work your way through error messages and configuration files!
>
> The compositor is still not _mature_ yet, any feedback and reports would be greatly appreciated!
> Please do so on the [GitHub issue tracker](https://github.com/nferhat/fht-compositor/issues/new?template=Blank+issue)
> or by joining the [matrix channel](https://matrix.to/#/#fht-compositor:matrix.org)
