use super::{WheelStep, WheelSteps};

/// Points per `scrollingDeltaY` tick of a wheel that does not report exact
/// distances; see the module docs for where 40 comes from.
const TICK: f32 = 40.0;

pub fn wheel_step() -> WheelSteps {
    WheelSteps { vertical: WheelStep::Points(TICK), horizontal: WheelStep::Points(TICK) }
}
