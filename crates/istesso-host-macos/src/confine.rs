//! Where an input for a remote session is allowed to land.
//!
//! A Mac host runs the session's application on the same login session as the
//! person's own desktop. Input that came from the device being shown the
//! session must reach that one application and nothing else — not another
//! application, not the lock screen. This module is the decision alone: given
//! the state of the Mac and one input, it says where the input may be
//! delivered, or that it is dropped and for what reason. It posts nothing and
//! needs no permission, so it is tested on any machine.
//!
//! The posting itself (`crate::input::post`, macOS only) must ask this module
//! first and obey it. The reasons for the rule are in docs/m4-plan.md 5c and
//! ADR 0017.

/// A process, by its id.
pub type Pid = i32;

/// A rectangle on the desktop, in points from the top left of the main screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// One on-screen window, as the window list reports it (owner process, layer,
/// bounds, how opaque). Only these fields are read; window names are not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub owner: Pid,
    pub layer: i32,
    pub bounds: Rect,
    /// 1.0 opaque, 0.0 fully transparent.
    pub alpha: f64,
}

/// A window below this is treated as not there for the top-window test.
const SEE_THROUGH: f64 = 0.05;

/// What kind of input is being delivered, and for a pointer event, where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// A key or a modifier. A key never carries a screen position.
    Key,
    /// A pointer move, button or scroll, at this desktop point.
    Pointer { x: f64, y: f64 },
}

/// The state of the Mac when the input is about to be delivered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State<'a> {
    /// The screen is locked. This is the ordinary state of a Mac host
    /// (docs/m4-plan.md 4.4): input must still reach the application, so
    /// locked is not by itself a reason to drop.
    pub locked: bool,
    /// The process of the application this session shows, if there is one.
    pub session_app: Option<Pid>,
    /// The process that is frontmost now.
    pub frontmost: Option<Pid>,
    /// Every on-screen window, front to back. Used for the pointer's
    /// top-window test on the session-tap path.
    pub windows: &'a [Window],
}

/// Where an input may be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Straight to this process (`CGEventPostToPid`). The session tap is not
    /// used, so the event does not move the host's own pointer and cannot
    /// reach another application or the lock screen.
    ToProcess(Pid),
    /// At the session's input tap (`CGEventPost`): the weaker form. It moves
    /// the host's real pointer and the window server routes it. Reached only
    /// when direct delivery is off, and never while locked.
    SessionTap,
    /// Not delivered. Counted by reason.
    Drop(DropReason),
}

/// Why an input was not delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// There is no application for this session yet.
    NoSessionApp,
    /// The session's application is not frontmost, and this path needs it.
    NotFrontmost,
    /// The session tap is never used while the screen is locked.
    LockedSessionTap,
    /// The topmost window under the pointer belongs to another process.
    OtherWindowOnTop,
    /// No window is under the pointer.
    NoWindowUnderPoint,
}

/// The policy. Direct-to-process delivery is preferred (`direct` on): it is
/// the only path used while locked, and it removes the gap that the session
/// tap leaves. The session tap is the fallback, used only if the device
/// cannot drive the application directly, and only while unlocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confine {
    /// Deliver straight to the application's process. When off, the session
    /// tap is used instead (unlocked only, with the top-window test).
    pub direct: bool,
    /// Whether direct delivery needs the application to be frontmost. Two
    /// sessions on one host need it off; whether a non-frontmost application
    /// takes keys and clicks is what the check program answers (5c).
    pub direct_needs_frontmost: bool,
}

impl Default for Confine {
    fn default() -> Self {
        // Until the check program shows direct delivery arrives, the safe
        // default delivers directly and does not require the front, and is
        // never allowed to fall through to the session tap while locked.
        Confine {
            direct: true,
            direct_needs_frontmost: false,
        }
    }
}

impl Confine {
    /// Where this input may go, in this state.
    pub fn decide(&self, item: Item, state: &State<'_>) -> Delivery {
        let Some(app) = state.session_app else {
            return Delivery::Drop(DropReason::NoSessionApp);
        };
        if self.direct {
            if self.direct_needs_frontmost && state.frontmost != Some(app) {
                return Delivery::Drop(DropReason::NotFrontmost);
            }
            return Delivery::ToProcess(app);
        }
        // The session tap: unlocked only, application frontmost, and for a
        // pointer the topmost window under it must be the application's.
        if state.locked {
            return Delivery::Drop(DropReason::LockedSessionTap);
        }
        if state.frontmost != Some(app) {
            return Delivery::Drop(DropReason::NotFrontmost);
        }
        match item {
            Item::Key => Delivery::SessionTap,
            Item::Pointer { x, y } => match top_window_at(state.windows, x, y) {
                Some(window) if window.owner == app => Delivery::SessionTap,
                Some(_) => Delivery::Drop(DropReason::OtherWindowOnTop),
                None => Delivery::Drop(DropReason::NoWindowUnderPoint),
            },
        }
    }
}

/// The frontmost window that contains the point and is not see-through.
fn top_window_at(windows: &[Window], x: f64, y: f64) -> Option<&Window> {
    windows
        .iter()
        .find(|window| window.alpha > SEE_THROUGH && window.bounds.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
        }
    }

    const APP: Pid = 501;
    const OTHER: Pid = 777;

    fn window(owner: Pid, bounds: Rect) -> Window {
        Window {
            owner,
            layer: 0,
            bounds,
            alpha: 1.0,
        }
    }

    fn state<'a>(locked: bool, frontmost: Pid, windows: &'a [Window]) -> State<'a> {
        State {
            locked,
            session_app: Some(APP),
            frontmost: Some(frontmost),
            windows,
        }
    }

    #[test]
    fn direct_delivery_goes_to_the_application_locked_or_not() {
        let confine = Confine::default();
        for locked in [false, true] {
            let state = state(locked, OTHER, &[]);
            assert_eq!(
                confine.decide(Item::Key, &state),
                Delivery::ToProcess(APP),
                "locked={locked}"
            );
            assert_eq!(
                confine.decide(Item::Pointer { x: 10.0, y: 10.0 }, &state),
                Delivery::ToProcess(APP),
            );
        }
    }

    #[test]
    fn direct_delivery_can_require_the_front_and_then_drops_when_another_is_front() {
        let confine = Confine {
            direct_needs_frontmost: true,
            ..Confine::default()
        };
        assert_eq!(
            confine.decide(Item::Key, &state(true, APP, &[])),
            Delivery::ToProcess(APP),
        );
        assert_eq!(
            confine.decide(Item::Key, &state(true, OTHER, &[])),
            Delivery::Drop(DropReason::NotFrontmost),
        );
    }

    #[test]
    fn nothing_is_delivered_when_there_is_no_session_application() {
        let confine = Confine::default();
        let state = State {
            session_app: None,
            ..state(false, APP, &[])
        };
        assert_eq!(
            confine.decide(Item::Key, &state),
            Delivery::Drop(DropReason::NoSessionApp),
        );
    }

    #[test]
    fn the_session_tap_is_never_used_while_locked() {
        let confine = Confine {
            direct: false,
            ..Confine::default()
        };
        let windows = [window(APP, rect(0.0, 0.0, 100.0, 100.0))];
        assert_eq!(
            confine.decide(Item::Key, &state(true, APP, &windows)),
            Delivery::Drop(DropReason::LockedSessionTap),
        );
        assert_eq!(
            confine.decide(
                Item::Pointer { x: 10.0, y: 10.0 },
                &state(true, APP, &windows)
            ),
            Delivery::Drop(DropReason::LockedSessionTap),
        );
    }

    #[test]
    fn the_session_tap_needs_the_application_frontmost() {
        let confine = Confine {
            direct: false,
            ..Confine::default()
        };
        let windows = [window(APP, rect(0.0, 0.0, 100.0, 100.0))];
        assert_eq!(
            confine.decide(Item::Key, &state(false, OTHER, &windows)),
            Delivery::Drop(DropReason::NotFrontmost),
        );
        assert_eq!(
            confine.decide(Item::Key, &state(false, APP, &windows)),
            Delivery::SessionTap,
        );
    }

    #[test]
    fn a_pointer_through_the_session_tap_needs_the_application_window_on_top() {
        let confine = Confine {
            direct: false,
            ..Confine::default()
        };
        let here = rect(0.0, 0.0, 100.0, 100.0);
        // The application's window alone: allowed.
        let own = [window(APP, here)];
        assert_eq!(
            confine.decide(Item::Pointer { x: 50.0, y: 50.0 }, &state(false, APP, &own)),
            Delivery::SessionTap,
        );
        // Another process's window in front of it at the point: dropped.
        let covered = [window(OTHER, here), window(APP, here)];
        assert_eq!(
            confine.decide(
                Item::Pointer { x: 50.0, y: 50.0 },
                &state(false, APP, &covered)
            ),
            Delivery::Drop(DropReason::OtherWindowOnTop),
        );
        // No window under the point: dropped.
        assert_eq!(
            confine.decide(
                Item::Pointer { x: 500.0, y: 500.0 },
                &state(false, APP, &own)
            ),
            Delivery::Drop(DropReason::NoWindowUnderPoint),
        );
    }

    #[test]
    fn a_see_through_window_in_front_does_not_count_as_on_top() {
        let confine = Confine {
            direct: false,
            ..Confine::default()
        };
        let here = rect(0.0, 0.0, 100.0, 100.0);
        let ghost = Window {
            alpha: 0.0,
            ..window(OTHER, here)
        };
        let windows = [ghost, window(APP, here)];
        assert_eq!(
            confine.decide(
                Item::Pointer { x: 50.0, y: 50.0 },
                &state(false, APP, &windows)
            ),
            Delivery::SessionTap,
        );
    }
}
