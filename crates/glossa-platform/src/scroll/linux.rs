use super::{WheelStep, WheelSteps};

/// Points per wheel click; see the module docs for where 53 comes from.
const CLICK: f32 = 53.0;

pub fn wheel_step() -> WheelSteps {
    WheelSteps { vertical: WheelStep::Points(CLICK), horizontal: WheelStep::Points(CLICK) }
}
