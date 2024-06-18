//! Signals.
//!
//! The configuration/scripts can register callbacks on certain signals, inspired by AwesomeWM,
//! like the following:
//!
//! ```lua
//! local callback_id = fht.register_callback("signal::name", function(data)
//!     -- `data` is a table, varies based on the signal,
//!     -- Now you can run actions based on conditions, for example
//!     if data.some_field == "some_name" then
//!         -- other calls, etc...
//!     end
//! end)
//! ```

use std::str::FromStr;

/// A signal the compositor can emit.
// TODO: Add more signals.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Signal {
    /// A simple test signal to see if the lua configuration receives signals.
    Test,
}

impl FromStr for Signal {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "test" => Ok(Self::Test),
            _ => Err(()),
        }
    }
}
