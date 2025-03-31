# Workspace system

`fht-compositor` has static workspaces assigned to each output/monitor.

There's no shared space/floating layer across outputs/workspaces. Each workspace is responsible to manage its own windows.
Windows in a workspace can not "overflow" to adjacent workspaces/monitors. This is very similar to what the [DWM](https://dwm.suckless.org)
and [AwesomeWM](https://awesomewm.org) window managers propose.  Effectively, you can think of each output as a sliding
carousel of 9 workspaces, displaying only one at a time.

When connecting outputs, the compositor will create a fresh set of workspaces for each one of them. When
disconnecting an output, all its windows will get inserted into the respective workspaces of the *primary output*
(most likely the first output you inserted, but this can be [configured]())

Compared to other wayland compositor/tiling window managers, workspaces **can not** be moved across outputs,
instead you move individual windows from/to workspaces or outputs.

## Advantages of this system

Windows are organized in a predictible fashion, and you have full knowledge and control on where they are and where they go.
Workspaces are always available for window rules and custom scripts (for example to create scratchpad workspaces).

## Example workflow

When I make use of the compositor, through muscle memory and [window rules](/configuration/window-rules), I
assign different workspaces different purposes: first workspace is the *work* one (that contains the project
I am currently *working* on), second is for the web browser, third is for chat clients (think Discord, Telegram,
Fractal, etc.) sixth is for video games, etc.

This streamlined workflow allows me to switch working context **very** fast using keybinds, since I have keybinds
engrained in my mind, I can switly get to my browser to search up documentation, quickly respond to a notification from
a chat client, and more.

If you want, you can even disable [animations](/configuration/animations) to make this even more immediate.
