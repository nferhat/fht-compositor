use smithay::delegate_background_effect;
use smithay::wayland::background_effect::ExtBackgroundEffectHandler;

use crate::state::State;

// No need to handle stuff here since we can just read from SurfaceCachedState
// Maybe logging but otherwise I don't see the point
impl ExtBackgroundEffectHandler for State {}

delegate_background_effect!(State);
