# `cursor` section

Cursor theme configuration.

If the compositor cannot find the right cursor theme with the right cursor size, it will use a default cursor image thats bundled with the compositor.

---

- `cursor.name`

The cursor theme name to use. Generally its the directory name of your cursor theme installed under `/usr/share/cursors/xorg-x11/{NAME}`

If this not set, the compositor will try to read `XCURSOR_NAME` if this field is not set.

---

- `cursor.size`

The cursor size to use. The cursor theme must has matching xcursor files with this size in order to display a cursor image.
