# Contribution guide

Some information about code style in the compositor.

First and foremost, formatting is done using `cargo +nightly fmt --all`, so make sure you pass your code through that
before committing/doing a PR.

### Code consistency
You should always read the code to a certain extend (depending on what you are about to work on) before starting to write,
this is especially important for new contributors, to get an idea about how the code is organized and how different modules
interact with each other.

When in doubt, you can always ask me (or someone that contributes regularly to the compositor) about what you should read to
get an idea about how "nferhat/whoever would do this". But for drafting purposes of a new feature, this can be an afterthought
and be ironed out in reviews
### Logging

`tracing` is the logging framework used, and their logging macros are `#[macro_use]` imported inside all the codebase.

Here is a break down of how you are expected to use them:

- `info!`: For information message, if some *important enough* action succeeded
- `warn!`: For unexpected behaviour, but ***not** important enough* to alter compositor activity
- `error!`: For unexpected behaviour, that is *important enough* to alter compositor activity
- `debug!`: For keeping track of events and actions that matter for developers, not end users

When writing/formatting your messages, you should:

- Avoid punctuation when logging messages
- use tracing's `?value` to specify arguments, unless it hurts user readability, for example `warn!(?err, "msg")`
