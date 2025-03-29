# Layouts

`fht-compositor` has dynamic layouts, IE. the workspace arranges windows for you, *automatically*.

Layouts divide windows in two stacks

- **Master stack**: Contains the most important window(s) that need the most attention. This is for example your opened
  text editor, or a mail client. The master window(s) take up a majority of the screen area.
- **Slave stack**: Contains the lesser important windows. This can be a video you have opened on the side, some
  reference material, library documentation, work terminals, etc. These windows share the remainder of the screen space.<br>
  It can be divided in more ways, for example the `centered-master` layout, that divides it in left and right stacks.

Different variables control the layout at runtime.

- **Number of master clients** (`nmaster`): The number of master clients in the master stack. Must always be >= 1
- **Master width factor** (`mwfact`): The proportion of screen space the master stack takes up relative to the slave stack. It
  in `[0.01, 0.99]`
- **Per-window proportion**: (`proportion`) The proportion control how much space a window takes relative to other windows in its
  stack.

By default, when you open a window in a workspace, it gets inserted as tiled at the *end* of the slave stack. The layout will
dynamically resize the other windows to share the screen space with the newly opened window.

## Floating windows

The layout system only affects windows that are *tiled*. Floating windows, on the other hand, don't get managed.

There's no such thing as a floating layer in a workspace. Floating windows live in the same layer the tiled windows, and thus
can be displayed *below* tiled windows.

You can make any window floating using the `float-focused-window` key action, or by making use of [window rules](/configuration/window-rules)

```toml
[[rules]]
# -> your matching rules here...
floating = true
centered = true # preference
```

The rendering order of windows is decided by their position in the workspace window list.

## Why dynamic layouts?

Dynamic layouts are extremely flexible and can be molded to adapt for any situation/workflow. You, *the end user*, don't have to
fuss creating a specialized layout for your current job in other window management models like Sway's or River's, instead, you
pick a layout that suits your need, resize and move some window around, and start working.

You can create workflows and make them work with what you have to do quickly, and adapt for some unforseen cases or patterns
the main task has. You can build from the provided layouts and get started working.

Overall, the experience is very natural and you'll get the hang of it really fast.
