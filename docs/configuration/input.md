# Input device configuration

You can configure various input device functionality under `fht-compositor` thanks to [libinput](https://www.freedesktop.org/wiki/Software/libinput/).

## Keyboard configuration

> [!NOTE]
> As of writing this, keyboard configuration is **only global**, IE. you can't set it per-device. This is a limitation of the
> wl-seat protocol, allowing for only one configuration at a time.

#### `rules`, `model`, `layout`, `variant`, `options`

These are all [XKB](https://www.x.org/wiki/XKB/) settings.

- You can find out available keyboard rules, variants and options from the `/usr/share/X11/xkb/rules/base.lst` file, or using `localectl` (see `man 5 localectl`)
- You can find the correct keyboard layout for yourself [on this page](https://xkeyboard-config.pages.freedesktop.org/website/layouts/gallery/)

By default, all these are empty strings (using system defaults), and layout is `us`

---

#### `repeat-rate`, `repeat-delay`

These two options control key repeating. `repeat-delay` is the delay in milliseconds that you should hown a key for key repeating to
start. `repeat-rate` is the frequency at which the key is repeated.

Default settings are `repeat-rate=25`, `repeat-delay=250`

## Mouse settings

Mouse settings are appliewd for regular mice, touchpads, trackballs, etc. The compositor (and libinput) will figure out automatically
which setting should be applied or not, and whether the connected mouse type supports a given feature.

> [!NOTE] Default mouse settings
> If an option does not have a default specified, it is up to the device driver (IE. libinput) to choose one.

---

#### `acceleration-profile`

How should the pointer cursor accelerate with mouse movement. Available values are:
- `adaptive`: Takes the current speed of the device into account when deciding on acceleration.
- `linear`: Constant factor `acceleration-speed` applied to all deltas, regardless of the speed of motion.

---

#### `acceleration-speed`

A factor to multiply mouse movement delta with. Must be in the range `[-1.0, 1.0]`

Default is `1.0`

---

#### `left-handed`

Whether to enable left handed mode for the device. This will swap the left and right clicks.

---

#### `scroll-method`

For touchpads, determines how to emulate a scroll wheel using only your fingers (and no dedicated button). Available values are:
- `no-scroll`: Disable scrolling emulation.
- `two-finger`: Scrolling is triggered by two fingers being placed on the surface of the touchpad.
- `edge`: Scrolling is triggered by moving a single finger along the right edge (vertical scroll) or bottom edge (horizontal scroll).
- `on-button-down`: Converts the motion of a device into scroll events while a designated button is held down. This is common in ThinkPad trackpoints

---

#### `scroll-button`, `scroll-button-lock`

The button used to enable `on-button-down` scroll method. When `scroll-button-lock` is enabled, the button does not need to be held
down, and insteads turns the button into a toggle switch.

---

#### `click-method`

Determines how button events are triggered on a touchpad/[clickpad](https://wayland.freedesktop.org/libinput/doc/latest/clickpad-softbuttons.html#clickpad-softbuttons)s.
Available values are:

- `button-areas`: The bottom area is divided into three thirds, like the following:
<p align=center> <img src="/assets/software-buttons-visualized.svg" /> </p>

- `click-finger`: Emulate clicks based on the number of fingers used, 1 is left, 2 is right, 3 is middle.

---

#### `natural-scrolling`

Whether to enable natural scrolling.

Natural scrolling matches the motion of the scroll device with the motion of the **content**

---

#### `middle-button-emulation`

Whether to emulate a left+right click at the same time as a middle click. The middle click is the one you have when you
click on your mouse's scroll wheel.

---

#### `disable-while-typing`

The name is clear enough.

---

#### `tap-to-click`, `tap-button-map`

Whether to emulate clicking on touchpads/clickpads by tapping the surface.

`tap-button-map` changes how tap-to-click behaves. Available maps/modes are: `left-right-middle`, `left-middle-right` (for
1 finger, 2 finger and 3 finger taps respectively)

---

#### `tap-and-drag`, `drag-lock`

Whether to enable Tap-and-drag. If a tap is shortly followed by the finger being held down, moving the finger around will
*drag around* the selected item from the tap.

Having `drag-lock` enabled will make the dragging process persist even when lifting the finger from the touchpad, and instead
will require a final tap to let go of the grabbed item.

## Per-device configuration

You can configure each registered input device individually. Per-device configuration is a table, which keys can be:
- The device pretty name (AKA. the readable name, which you would see in a device manager)
- A raw device path, `/dev/input/eventX`

To find out which devices you have connected, you can execute in a shell <br>
```sh
# You might need root privileges to run this
libinput list-devices | grep Device:
```

---

- `disable`: Whether to completely disable this device.
- `mouse`: Same as `input.mouse`, but for this device only.
