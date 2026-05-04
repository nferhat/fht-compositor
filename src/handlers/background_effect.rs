use smithay::delegate_background_effect;
use smithay::wayland::background_effect::ExtBackgroundEffectHandler;

use crate::state::State;

impl ExtBackgroundEffectHandler for State {}

delegate_background_effect!(State);
