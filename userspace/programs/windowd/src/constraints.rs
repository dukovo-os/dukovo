// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The pointer held to a window, and a mouse's own motion
//! (pointer-constraints and relative-pointer, ADR 0728).
//!
//! A program that turns a view with the mouse, as a game does, has no
//! use for a pointer that stops at the screen's edge. It asks for two
//! things. One is the mouse's motion as the mouse made it: every motion
//! is told to the surface with the pointer both as far as the pointer
//! went for it and as the mouse's own counts. The other is a constraint
//! on the pointer for one of its surfaces: locked, the pointer stays
//! where it is; confined, it stays on the surface, or on a region of
//! it.
//!
//! A constraint is the program's wish, and windowd says when it holds:
//! while the surface has the pointer, its window is the one the person
//! works in, and the window manager has no hold of the pointer. So the
//! person always has a way out: another window by the keys, or the list
//! of windows. A constraint the program asked for once is gone when it
//! stops holding; one it asked for to last holds again when the person
//! comes back.
//!
//! Smithay keeps the constraints, and ends the one of a surface the
//! pointer leaves. The desktop asks here, after everything it is told,
//! what holds the pointer now ([`takes`]), and follows that when a
//! mouse moves.

use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::input::pointer::PointerHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::pointer_constraints::{
    PointerConstraint, PointerConstraintsHandler, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::{delegate_pointer_constraints, delegate_relative_pointer};

use crate::shell::Shell;

/// The two protocols' globals, which stay for as long as the display
/// does.
#[derive(Debug)]
pub struct Offered {
    _constraints: PointerConstraintsState,
    _relative: RelativePointerManagerState,
}

/// Offers pointer-constraints and relative-pointer on `display`.
pub fn offer(display: &DisplayHandle) -> Offered {
    Offered {
        _constraints: PointerConstraintsState::new::<Shell>(display),
        _relative: RelativePointerManagerState::new::<Shell>(display),
    }
}

/// How a surface holds the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The pointer stays where it is.
    Locked {
        /// Where the program would have the pointer when the lock ends,
        /// from the surface's corner, if it said.
        hint: Option<Point<f64, Logical>>,
    },
    /// The pointer stays on the surface.
    Confined,
}

/// The surface that holds the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub surface: WlSurface,
    /// Where the surface's top left corner is on the desktop.
    pub at: Point<f64, Logical>,
    pub kind: Kind,
}

impl Held {
    /// Whether `other` is the same hold: of the same surface, in the
    /// same way.
    pub fn same(&self, other: &Held) -> bool {
        let locked = |held: &Held| matches!(held.kind, Kind::Locked { .. });
        self.surface == other.surface && locked(self) == locked(other)
    }
}

/// How `surface` holds `pointer`, which is over it at the place
/// `within` of it and may be held now. A constraint that is not in
/// force yet is from here on, if the pointer is where its program asked
/// for it.
pub fn takes(
    surface: &WlSurface,
    pointer: &PointerHandle<Shell>,
    within: Point<f64, Logical>,
) -> Option<Kind> {
    with_pointer_constraint(surface, pointer, |constraint| {
        let constraint = constraint?;
        if !constraint.is_active() {
            let asked = constraint.region();
            if asked.is_some_and(|region| !region.contains(within.to_i32_floor::<i32>())) {
                return None;
            }
            constraint.activate();
        }
        Some(match &*constraint {
            PointerConstraint::Locked(locked) => {
                Kind::Locked { hint: locked.cursor_position_hint() }
            }
            PointerConstraint::Confined(_) => Kind::Confined,
        })
    })
}

/// The constraint of `surface` on `pointer` holds no longer, if it did:
/// its program is told, and one that was asked for once is gone.
pub fn ends(surface: &WlSurface, pointer: &PointerHandle<Shell>) {
    // A surface that is gone has no constraint to end.
    if !surface.is_alive() {
        return;
    }
    with_pointer_constraint(surface, pointer, |constraint| {
        if let Some(constraint) = constraint.filter(|constraint| constraint.is_active()) {
            constraint.deactivate();
        }
    });
}

/// Whether the constraint of `surface` on `pointer` lets the pointer be
/// at the place `within` of the surface: in the region its program
/// named, or anywhere if it named none.
pub fn lets(
    surface: &WlSurface,
    pointer: &PointerHandle<Shell>,
    within: Point<f64, Logical>,
) -> bool {
    with_pointer_constraint(surface, pointer, |constraint| {
        let asked = constraint.as_ref().and_then(|constraint| constraint.region());
        asked.is_none_or(|region| region.contains(within.to_i32_floor::<i32>()))
    })
}

/// The width and height of `surface`, once it is drawn.
pub fn size_of(surface: &WlSurface) -> Option<Size<i32, Logical>> {
    with_states(surface, |states| {
        let drawn = states.data_map.get::<RendererSurfaceStateUserData>()?;
        drawn.lock().ok()?.surface_size()
    })
}

/// How many times the way to a place the pointer may not go to is
/// halved to find how far along it the pointer may go: to well within a
/// pixel for any motion a mouse makes.
const HALVINGS: u32 = 16;

/// Where a pointer at `from` comes to that is to go to `to` and is
/// held to `bounds`, and in them to the places `allowed` takes. It goes
/// as far towards `to` as it may, and then on across and down, each as
/// far as it may by itself: along an edge it cannot cross it slides.
pub fn confined(
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    bounds: Rectangle<f64, Logical>,
    allowed: impl Fn(Point<f64, Logical>) -> bool,
) -> Point<f64, Logical> {
    // The last place of each row and column is a whole pixel in from
    // the far edge, as on a screen.
    let last_x = (bounds.loc.x + bounds.size.w - 1.0).max(bounds.loc.x);
    let last_y = (bounds.loc.y + bounds.size.h - 1.0).max(bounds.loc.y);
    let to = Point::from((to.x.clamp(bounds.loc.x, last_x), to.y.clamp(bounds.loc.y, last_y)));
    let straight = furthest(from, to, &allowed);
    let across = furthest(straight, (to.x, straight.y).into(), &allowed);
    furthest(across, (across.x, to.y).into(), &allowed)
}

/// The place furthest along the straight way from `from` to `to` that
/// `allowed` takes, with every place before it: `from` itself if none
/// further is.
fn furthest(
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    allowed: &impl Fn(Point<f64, Logical>) -> bool,
) -> Point<f64, Logical> {
    if allowed(to) {
        return to;
    }
    let along = |part: f64| from + (to - from).upscale(part);
    let (mut reached, mut refused) = (0.0, 1.0);
    for _ in 0..HALVINGS {
        let half = (reached + refused) / 2.0;
        if allowed(along(half)) {
            reached = half;
        } else {
            refused = half;
        }
    }
    along(reached)
}

impl PointerConstraintsHandler for Shell {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Shell>) {
        // Whether it holds is settled with everything else the programs
        // asked, when the desktop looks ([`takes`]).
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        _pointer: &PointerHandle<Shell>,
        location: Point<f64, Logical>,
    ) {
        // The lock may be gone before the desktop next looks: a program
        // says where it would have the pointer, commits, and lets go of
        // the lock, all at once.
        if let Some(held) = self.held.as_mut().filter(|held| held.surface == *surface)
            && let Kind::Locked { hint } = &mut held.kind
        {
            *hint = Some(location);
        }
    }
}

delegate_pointer_constraints!(Shell);
delegate_relative_pointer!(Shell);

#[cfg(test)]
mod tests {
    use super::*;

    fn place(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    #[test]
    fn a_confined_pointer_stops_at_the_edge_and_slides_along_it() {
        let bounds = Rectangle::new(place(100.0, 50.0), (200.0, 100.0).into());
        let anywhere = |_| true;
        let goes = |from, to| confined(from, to, bounds, anywhere);
        assert_eq!(goes(place(150.0, 60.0), place(160.0, 70.0)), place(160.0, 70.0));
        // Past the right edge and the bottom: the last place of each.
        assert_eq!(goes(place(150.0, 60.0), place(400.0, 70.0)), place(299.0, 70.0));
        assert_eq!(goes(place(150.0, 60.0), place(90.0, 500.0)), place(100.0, 149.0));
    }

    #[test]
    fn a_place_that_is_not_allowed_is_gone_up_to_and_then_along() {
        let bounds = Rectangle::new(place(0.0, 0.0), (100.0, 100.0).into());
        // The left half alone is allowed: the pointer goes up to its
        // edge, and all the way down.
        let left = |at: Point<f64, Logical>| at.x < 50.0;
        let to = confined(place(40.0, 10.0), place(60.0, 30.0), bounds, left);
        assert!(to.x > 49.9 && to.x < 50.0 && to.y == 30.0, "{to:?}");
        // The top half alone: all the way across.
        let top = |at: Point<f64, Logical>| at.y < 50.0;
        let to = confined(place(40.0, 40.0), place(60.0, 70.0), bounds, top);
        assert!(to.x == 60.0 && to.y > 49.9 && to.y < 50.0, "{to:?}");
        // Nowhere it may go: it stays.
        let nowhere = |_| false;
        assert_eq!(
            confined(place(40.0, 40.0), place(60.0, 70.0), bounds, nowhere),
            place(40.0, 40.0)
        );
    }

    #[test]
    fn bounds_with_no_room_hold_the_pointer_to_their_corner() {
        let bounds = Rectangle::new(place(10.0, 10.0), (0.0, 0.5).into());
        let to = confined(place(10.0, 10.0), place(30.0, 30.0), bounds, |_| true);
        assert_eq!(to, place(10.0, 10.0));
    }
}
