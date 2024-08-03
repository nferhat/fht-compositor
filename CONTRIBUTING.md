# Contribution guide

Some insight and guidelines for contribution to the compositor.

- Formatting is done using `cargo +nightly fmt`
- Current MSRV is `nightly`, due to [this](https://github.com/rust-lang/rust/issues/95439)
- Keep your git commit titles short, expand in their descriptions (your editor should have settings for this)

## Logging

There exist four kind of log levels:
- `info!`: For information message, if some *important enough* action succeeded
- `warn!`: For error/unexpected behaviour, but ***not** important enough* to alter compositor activity
- `error!`: For error/unexpected behaviour, that is *important enough* to alter compositor activity
- `debug!`: For keeping track of events and actions that matter for developers, not end users

Additional directives are
- Avoid punctuation when logging messages
- use tracing's `?value` to specify arguments, unless it hurts user readability, for example `warn!(?err, "msg")`

## Code organization

- `backend::*`: Backend-only interaction
- `config::*`: Config types
- `handlers::*`: Custom `*Handler` trait types or `delegate_*` macros, required by smithay.
- `portals::*`: [XDG desktop portals](https://flatpak.github.io/xdg-desktop-portal/)
- `renderer::*`: Rendering and custom render elements
- `shell::*`: Modules related to the desktop shell with `xdg-shell`, `wlr-layer-shell`, workspaces, etc.
- `utils::*`: General enough utilities (optimally I'd get rid of this)
