//! Scrolling that moves like the system's: a wheel notch goes as far as it
//! does in native programs, and a touchpad swipe holds the axis it started
//! on, so a vertical swipe does not drift sideways through a page.
//!
//! egui scrolls a fixed 40 points per wheel "line" on every system and lets
//! both axes of a touchpad gesture through. [`install`] adds a plugin that
//! rewrites each window's wheel events before egui reads them, so every
//! `ScrollArea` gets the change without knowing about it:
//!
//! - a wheel line becomes the system's step in points
//!   ([`glossa_platform::scroll::wheel_step`]); egui still eases large steps
//!   over a few frames, and passes small touchpad deltas straight through;
//! - a touchpad gesture (deltas in points, or fractions of a line where the
//!   system reports touchpads as wheels, as Windows does) is held to its
//!   dominant axis until it ends, momentum included. Whole wheel notches
//!   are left alone: a tilt wheel turns sideways when it is told to;
//! - with the zoom modifier held (Ctrl, Cmd on macOS) nothing is changed:
//!   egui turns the wheel into `zoom_delta` there, which the text and
//!   keyboard zoom read, and that should not grow with the scroll step.
//!
//! The axis lock follows `fastframe-scroll`
//! (<https://github.com/crmne/fastframe>, MIT, Copyright (c) 2026 Carmine
//! Paolino), moved from the frame's smoothed delta to the raw events so
//! egui's own smoothing never sees the cross-axis drift. Its touchpad
//! momentum and scaling for Linux are not taken: they repaint every frame
//! while gliding, and Linux has not been tried on real hardware yet.

use std::collections::HashMap;

use egui::{Event, MouseWheelSource, MouseWheelUnit, RawInput, TouchPhase, Vec2, ViewportId};
use glossa_platform::scroll::{wheel_step, WheelStep, WheelSteps};

/// Adds the scrolling plugin to a new context.
pub fn install(ctx: &egui::Context) {
    ctx.add_plugin(NativeScroll::default());
}

/// The system's steps are read again when a scroll starts after this long
/// without one, in seconds, so a changed setting applies to the next
/// scroll without a read per event.
const REREAD_AFTER: f64 = 1.0;
/// Where the system does not say when fingers touch and lift (Windows,
/// X11), a gesture that has not moved along its axis for this long, in
/// seconds, is over.
const GESTURE_GAP: f64 = 0.15;
/// The axis is decided once a gesture has travelled this far, in points;
/// before that each event follows the travel so far.
const DECIDE_AFTER: f32 = 4.0;
/// Travel this much wider than tall makes a sideways gesture: vertical
/// scrolling is the common case and wins a near-diagonal start.
const SIDEWAYS: f32 = 1.2;
/// Where the system announces touches (macOS, Wayland), a gesture with no
/// event for this long, in seconds, is over even without its lift: winit
/// on Wayland sends a touch for some wheels but never the lift. The same
/// time egui gives resting fingers.
const RESTING: f64 = 1.0;
/// Windows' "one screen at a time" scrolls the control's height; the scroll
/// area under the pointer is not known before egui lays it out, so a page
/// here is the window's height (as egui's own `MouseWheelUnit::Page` is),
/// minus a little so the last line stays in view.
const PAGE_FRACTION: f32 = 0.875;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

struct NativeScroll {
    /// Reads the system's steps ([`wheel_step`]; a fixed answer in tests).
    read: fn() -> WheelSteps,
    steps: Option<WheelSteps>,
    /// Per window: egui-winit keeps a clock per viewport, so times are only
    /// comparable within one.
    windows: HashMap<ViewportId, WindowScroll>,
}

struct WindowScroll {
    /// Input time of the latest wheel event.
    last_event: f64,
    /// The window's height and width in points, for a page step.
    size: Vec2,
    /// The system announced the touch that began the gesture in progress
    /// and has not announced its lift, so a pause with fingers resting is
    /// not taken for the end of it.
    announced: bool,
    /// The axis the gesture holds, once decided.
    axis: Option<Axis>,
    /// How far the gesture has gone before its axis was decided.
    travel: Vec2,
    /// Input time of the latest movement along the held axis.
    last_move: f64,
}

impl Default for NativeScroll {
    fn default() -> Self {
        NativeScroll { read: wheel_step, steps: None, windows: HashMap::new() }
    }
}

impl Default for WindowScroll {
    fn default() -> Self {
        WindowScroll {
            last_event: f64::NEG_INFINITY,
            size: Vec2::new(600.0, 600.0),
            announced: false,
            axis: None,
            travel: Vec2::ZERO,
            last_move: f64::NEG_INFINITY,
        }
    }
}

impl egui::Plugin for NativeScroll {
    fn debug_name(&self) -> &'static str {
        "glossa::scroll"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut RawInput) {
        if !input.viewports.is_empty() {
            self.windows.retain(|id, _| input.viewports.contains_key(id));
        }
        let win = self.windows.entry(input.viewport_id).or_default();
        if let Some(rect) = input.screen_rect {
            win.size = rect.size();
        }
        if !input.events.iter().any(|e| matches!(e, Event::MouseWheel { .. })) {
            return;
        }
        let now = input.time.unwrap_or(win.last_event);
        if self.steps.is_none() || now - win.last_event > REREAD_AFTER {
            self.steps = Some((self.read)());
        }
        let steps = self.steps.expect("read above");
        let zoom = ctx.options(|o| o.input_options.zoom_modifier);
        for event in &mut input.events {
            if let Event::MouseWheel { unit, delta, phase, source, modifiers } = event {
                if modifiers.matches_any(zoom) {
                    win.last_event = now;
                    continue;
                }
                win.on_wheel(steps, now, unit, delta, *phase, *source);
            }
        }
    }
}

impl WindowScroll {
    fn on_wheel(
        &mut self,
        steps: WheelSteps,
        now: f64,
        unit: &mut MouseWheelUnit,
        delta: &mut Vec2,
        phase: TouchPhase,
        source: MouseWheelSource,
    ) {
        let quiet = now - self.last_event;
        self.last_event = now;
        if self.announced && quiet > RESTING {
            self.announced = false;
            self.reset();
        }
        match phase {
            TouchPhase::Start => {
                self.announced = true;
                // Momentum after the fingers lift carries the same gesture on.
                if source != MouseWheelSource::Momentum {
                    self.reset();
                }
                return;
            }
            TouchPhase::End | TouchPhase::Cancel => {
                // The axis stays for the system's momentum, which follows
                // at once; after a quiet gap the next movement decides anew.
                self.announced = false;
                return;
            }
            TouchPhase::Move => {}
        }
        let notch = match *unit {
            MouseWheelUnit::Line => {
                let whole = delta.x.fract() == 0.0 && delta.y.fract() == 0.0;
                let notch = source == MouseWheelSource::Wheel || whole;
                *delta = Vec2::new(
                    delta.x * self.points(steps.horizontal, self.size.x),
                    delta.y * self.points(steps.vertical, self.size.y),
                );
                *unit = MouseWheelUnit::Point;
                notch
            }
            MouseWheelUnit::Point => false,
            MouseWheelUnit::Page => true,
        };
        if notch {
            return;
        }
        *delta = self.hold_axis(now, *delta);
    }

    fn points(&self, step: WheelStep, page: f32) -> f32 {
        match step {
            WheelStep::Points(p) => p,
            WheelStep::Page => page * PAGE_FRACTION,
        }
    }

    /// Keeps a gesture's movement on the axis it started on.
    fn hold_axis(&mut self, now: f64, delta: Vec2) -> Vec2 {
        if !self.announced && now - self.last_move > GESTURE_GAP {
            self.reset();
        }
        let axis = match self.axis {
            Some(axis) => axis,
            None => {
                self.travel += delta;
                let axis = if self.travel.x.abs() > self.travel.y.abs() * SIDEWAYS {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                if self.travel.length() >= DECIDE_AFTER {
                    self.axis = Some(axis);
                }
                axis
            }
        };
        let along = match axis {
            Axis::Horizontal => Vec2::new(delta.x, 0.0),
            Axis::Vertical => Vec2::new(0.0, delta.y),
        };
        // Only movement along the axis keeps the gesture alive: a wheel
        // that turns sideways right after a vertical swipe takes over once
        // the swipe has stopped.
        if along != Vec2::ZERO {
            self.last_move = now;
        }
        along
    }

    fn reset(&mut self) {
        self.axis = None;
        self.travel = Vec2::ZERO;
        self.last_move = f64::NEG_INFINITY;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(unit: MouseWheelUnit, delta: Vec2, phase: TouchPhase, source: MouseWheelSource) -> Event {
        Event::MouseWheel { unit, delta, phase, source, modifiers: Default::default() }
    }

    /// Runs the plugin over one frame's events at `time` and returns them.
    fn frame(p: &mut NativeScroll, time: f64, events: Vec<Event>) -> Vec<Vec2> {
        let ctx = egui::Context::default();
        let mut input = RawInput { time: Some(time), events, ..Default::default() };
        egui::Plugin::input_hook(p, &ctx, &mut input);
        input
            .events
            .iter()
            .filter_map(|e| match e {
                Event::MouseWheel { delta, phase: TouchPhase::Move, .. } => Some(*delta),
                _ => None,
            })
            .collect()
    }

    fn plugin(read: fn() -> WheelSteps) -> NativeScroll {
        NativeScroll { read, ..Default::default() }
    }

    fn points(p: f32) -> WheelSteps {
        WheelSteps { vertical: WheelStep::Points(p), horizontal: WheelStep::Points(p) }
    }

    #[test]
    fn a_notch_moves_by_the_system_step() {
        let mut p = plugin(|| points(100.0));
        let out = frame(
            &mut p,
            0.5,
            vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, -1.0), TouchPhase::Move, MouseWheelSource::Unknown)],
        );
        assert_eq!(out, vec![Vec2::new(0.0, -100.0)]);
    }

    #[test]
    fn a_page_step_scrolls_most_of_the_window() {
        let mut p = plugin(|| WheelSteps { vertical: WheelStep::Page, horizontal: WheelStep::Points(10.0) });
        p.windows.entry(ViewportId::ROOT).or_default().size = Vec2::new(300.0, 400.0);
        let out = frame(
            &mut p,
            0.5,
            vec![wheel(MouseWheelUnit::Line, Vec2::new(0.0, 1.0), TouchPhase::Move, MouseWheelSource::Unknown)],
        );
        assert_eq!(out, vec![Vec2::new(0.0, 350.0)]);
    }

    #[test]
    fn a_vertical_swipe_does_not_drift_sideways() {
        let mut p = plugin(|| points(40.0));
        let pad = |x, y| wheel(MouseWheelUnit::Point, Vec2::new(x, y), TouchPhase::Move, MouseWheelSource::Trackpad);
        let start = wheel(MouseWheelUnit::Point, Vec2::ZERO, TouchPhase::Start, MouseWheelSource::Trackpad);
        frame(&mut p, 0.0, vec![start.clone()]);
        let out = frame(&mut p, 0.016, vec![pad(1.0, -5.0)]);
        assert_eq!(out, vec![Vec2::new(0.0, -5.0)]);
        // Later the fingers wander sideways more than down: still vertical.
        let out = frame(&mut p, 0.032, vec![pad(6.0, -2.0)]);
        assert_eq!(out, vec![Vec2::new(0.0, -2.0)]);
        // The lift and the system's momentum keep the axis.
        let end = wheel(MouseWheelUnit::Point, Vec2::ZERO, TouchPhase::End, MouseWheelSource::Trackpad);
        let glide = wheel(MouseWheelUnit::Point, Vec2::ZERO, TouchPhase::Start, MouseWheelSource::Momentum);
        frame(&mut p, 0.048, vec![end, glide]);
        let out = frame(
            &mut p,
            0.5,
            vec![wheel(MouseWheelUnit::Point, Vec2::new(3.0, -1.0), TouchPhase::Move, MouseWheelSource::Momentum)],
        );
        assert_eq!(out, vec![Vec2::new(0.0, -1.0)]);
        // A new touch decides afresh.
        frame(&mut p, 1.0, vec![start]);
        let out = frame(&mut p, 1.016, vec![pad(8.0, 1.0)]);
        assert_eq!(out, vec![Vec2::new(8.0, 0.0)]);
    }

    #[test]
    fn touchpad_reported_as_fractional_lines_is_held_too() {
        // Windows: a precision touchpad sends small wheel deltas, the two
        // axes in separate messages, with no touch or lift.
        let mut p = plugin(|| points(30.0));
        let line = |x, y| wheel(MouseWheelUnit::Line, Vec2::new(x, y), TouchPhase::Move, MouseWheelSource::Unknown);
        let out = frame(&mut p, 0.0, vec![line(0.0, -0.2), line(0.05, 0.0), line(0.0, -0.1)]);
        assert_eq!(out, vec![Vec2::new(0.0, -6.0), Vec2::ZERO, Vec2::new(0.0, -3.0)]);
        // After a pause the next gesture may go sideways.
        let out = frame(&mut p, 0.5, vec![line(0.2, 0.0)]);
        assert_eq!(out, vec![Vec2::new(6.0, 0.0)]);
    }

    #[test]
    fn a_tilt_notch_is_never_held() {
        let mut p = plugin(|| points(30.0));
        let out = frame(
            &mut p,
            0.0,
            vec![
                wheel(MouseWheelUnit::Line, Vec2::new(0.0, -0.3), TouchPhase::Move, MouseWheelSource::Unknown),
                wheel(MouseWheelUnit::Line, Vec2::new(1.0, 0.0), TouchPhase::Move, MouseWheelSource::Unknown),
            ],
        );
        assert_eq!(out, vec![Vec2::new(0.0, -9.0), Vec2::new(30.0, 0.0)]);
    }

    fn pad(x: f32, y: f32) -> Event {
        wheel(MouseWheelUnit::Point, Vec2::new(x, y), TouchPhase::Move, MouseWheelSource::Trackpad)
    }

    fn phase(phase: TouchPhase) -> Event {
        wheel(MouseWheelUnit::Point, Vec2::ZERO, phase, MouseWheelSource::Trackpad)
    }

    #[test]
    fn the_zoom_modifier_leaves_the_wheel_to_egui() {
        // Ctrl+wheel zooms through egui's `zoom_delta`, which grows with
        // the delta: a notch must stay one line, whatever the scroll step.
        let mut p = plugin(|| WheelSteps { vertical: WheelStep::Page, horizontal: WheelStep::Page });
        let ctx = egui::Context::default();
        let zoom = ctx.options(|o| o.input_options.zoom_modifier);
        let ctrl = Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 1.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: zoom,
        };
        let pinchy = Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::new(2.0, 1.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Trackpad,
            modifiers: zoom,
        };
        let mut input = RawInput { time: Some(0.0), events: vec![ctrl.clone(), pinchy.clone()], ..Default::default() };
        egui::Plugin::input_hook(&mut p, &ctx, &mut input);
        assert_eq!(input.events, vec![ctrl, pinchy]);
    }

    #[test]
    fn a_lift_without_momentum_lets_the_next_gesture_choose() {
        let mut p = plugin(|| points(40.0));
        frame(&mut p, 0.0, vec![phase(TouchPhase::Start)]);
        assert_eq!(frame(&mut p, 0.016, vec![pad(1.0, -6.0)]), vec![Vec2::new(0.0, -6.0)]);
        frame(&mut p, 0.032, vec![phase(TouchPhase::End)]);
        // A new touch, announced.
        frame(&mut p, 0.3, vec![phase(TouchPhase::Start)]);
        assert_eq!(frame(&mut p, 0.316, vec![pad(6.0, 1.0)]), vec![Vec2::new(6.0, 0.0)]);
        frame(&mut p, 0.332, vec![phase(TouchPhase::End)]);
        // Movement after a quiet gap with no touch announced decides anew too.
        assert_eq!(frame(&mut p, 0.6, vec![pad(1.0, -6.0)]), vec![Vec2::new(0.0, -6.0)]);
    }

    #[test]
    fn a_touch_that_never_lifts_ends_after_a_rest() {
        // Wayland: winit sends a touch for some wheels, never the lift.
        let mut p = plugin(|| points(40.0));
        frame(&mut p, 0.0, vec![phase(TouchPhase::Start)]);
        assert_eq!(frame(&mut p, 0.016, vec![pad(0.0, -6.0)]), vec![Vec2::new(0.0, -6.0)]);
        // Fingers resting half a second still hold the axis.
        assert_eq!(frame(&mut p, 0.5, vec![pad(6.0, -1.0)]), vec![Vec2::new(0.0, -1.0)]);
        // Two seconds of nothing end the gesture.
        assert_eq!(frame(&mut p, 2.5, vec![pad(6.0, 1.0)]), vec![Vec2::new(6.0, 0.0)]);
        // And the 150 ms gap applies again, with no touch announced.
        assert_eq!(frame(&mut p, 2.8, vec![pad(1.0, -6.0)]), vec![Vec2::new(0.0, -6.0)]);
    }

    #[test]
    fn closed_windows_are_forgotten() {
        let mut p = plugin(|| points(40.0));
        let child = ViewportId::from_hash_of("options");
        let ctx = egui::Context::default();
        let mut both = RawInput { viewport_id: child, events: vec![pad(0.0, -6.0)], ..Default::default() };
        both.viewports.insert(ViewportId::ROOT, Default::default());
        both.viewports.insert(child, Default::default());
        egui::Plugin::input_hook(&mut p, &ctx, &mut both);
        assert!(p.windows.contains_key(&child));
        let mut root_only = RawInput::default();
        root_only.viewports.insert(ViewportId::ROOT, Default::default());
        egui::Plugin::input_hook(&mut p, &ctx, &mut root_only);
        assert!(!p.windows.contains_key(&child));
        assert!(p.windows.contains_key(&ViewportId::ROOT));
    }
}
