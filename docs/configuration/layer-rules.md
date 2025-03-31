# Layer-shell rules

Layer rules, similar to [window rules](./window-rules), are a way to apply custom settings on layer-shells.

Just like window rules, they have two parts: the match part, and the properties part. Refer to the [window rules]
page for information about what that is and `match-all` property.

For now, layer-shell rules are used to enable various effects on matched layer-shells

## The match part

#### `match-namespace`

A list of  [Regular Expression](https://en.wikipedia.org/wiki/Regular_expression)s. They match onto the layer-shell's namespace.
The namespace per protocol definition defines the purpose of a layer-shell, for example, `notification`, or `volume-osd`

Requires that the namespace has a match on only *one* regex from the given regexes.

#### `on-output`

Match on the output the layer-shell is opened on. Nothing fancy.

The following rule matches all layer-shells opened on a laptop's internal display

```toml
[[layer-rules]]
on-output = "eDP-1"
opacity = 0.5
```

## Layer-shell properties

#### `border`, `blur`, `shadow`

These values take the same fields as their versions in the [decorations configuration](/configuration/decorations),
however, they will *override* the decorations configuration with whatever fields you have provided.

By default, layer-shells have all of these disabled. Set `disable=false` to enable these effectss.

---

#### `opacity`

The opacity of the layer-shell, `0.0` is fully transparent, `1.0` is fully opaque.
