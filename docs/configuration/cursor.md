# Cursor theme.

`fht-compositor` supports XCursor themes. You can configured the used theme and the cursor:

```toml
[cursor]
name = "Vimix-cursors"
size = 32
```

If these are not specified, the compositor will try to load these values from the `XCURSOR_NAME`
and `XCURSOR_SIZE` variables.

When loading and applying the cursor theme, the compositor will set these variables.
