// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a screen's frame is drawn from.
//!
//! From the front: the panel windowd shows over the windows, then each
//! window in the windows' order, and of each window its popups, the
//! strips of its frame and its own surface. The surface is cut to the
//! place the frame's layout gives it, so that a program draws nowhere
//! else, and lies behind the frame, which is see-through where the
//! surface shows: the content, and the region of a title bar the
//! program shares (ADR 0719).

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::{Kind, render_elements};
use smithay::backend::renderer::pixman::{PixmanError, PixmanRenderer};
use smithay::desktop::PopupManager;
use smithay::utils::{Logical, Physical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;

use crate::panel::Panel;
use crate::windows::{Framed, Windows};

render_elements! {
    /// One thing a screen's frame is drawn from.
    pub Piece<=PixmanRenderer>;
    /// windowd's own pixels: a strip of a window's frame, or a panel.
    Own=MemoryRenderBufferRenderElement<PixmanRenderer>,
    /// A surface of a popup, whole.
    Popup=WaylandSurfaceRenderElement<PixmanRenderer>,
    /// A surface of a window, cut to the window's place in its frame.
    Window=CropRenderElement<WaylandSurfaceRenderElement<PixmanRenderer>>,
}

/// What the screens show beside the windows.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// The windows.
    pub windows: &'a Windows,
    /// The panel over them.
    pub panel: Option<&'a Panel>,
}

/// The place of the desktop `at` as a place of the frame of the screen
/// whose top left corner is at `screen`. A screen shows the desktop at
/// its own size in release 1.1.
fn on_screen(at: Point<i32, Logical>, screen: Point<i32, Logical>) -> Point<i32, Physical> {
    (at - screen).to_physical(1)
}

impl Scene<'_> {
    /// What the frame of the screen whose area of the desktop is
    /// `screen` is drawn from, from the front to the back.
    ///
    /// # Errors
    ///
    /// If the renderer cannot take in pixels of windowd's own.
    pub fn pieces(
        &self,
        renderer: &mut PixmanRenderer,
        screen: Rectangle<i32, Logical>,
    ) -> Result<Vec<Piece>, PixmanError> {
        let mut pieces = Vec::new();
        if let Some(panel) = self.panel {
            let at = on_screen(panel.area().loc, screen.loc).to_f64();
            let own = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                at,
                panel.pixels(),
                None,
                None,
                None,
                Kind::Unspecified,
            )?;
            pieces.push(Piece::Own(own));
        }
        for framed in self.windows.stacked() {
            window(renderer, framed, screen.loc, &mut pieces)?;
        }
        Ok(pieces)
    }
}

/// Adds to `pieces` what `framed` is drawn from on the screen whose top
/// left corner is at `screen`.
fn window(
    renderer: &mut PixmanRenderer,
    framed: &Framed,
    screen: Point<i32, Logical>,
    pieces: &mut Vec<Piece>,
) -> Result<(), PixmanError> {
    let Some(surface) = framed.window.wl_surface() else { return Ok(()) };
    let origin = on_screen(framed.surface_at(), screen);
    for (popup, within) in PopupManager::popups_for_surface(&surface) {
        let offset = framed.window.geometry().loc + within - popup.geometry().loc;
        pieces.extend(render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            origin + offset.to_physical(1),
            1.0,
            1.0,
            Kind::Unspecified,
        ));
    }
    for strip in framed.strips() {
        let at = framed.at() + Point::from((strip.rect.x, strip.rect.y));
        let own = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            on_screen(at, screen).to_f64(),
            &strip.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )?;
        pieces.push(Piece::Own(own));
    }
    let shows = framed.surface_shows();
    let shows = Rectangle::new(on_screen(shows.loc, screen), shows.size.to_physical(1));
    let whole: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> =
        render_elements_from_surface_tree(renderer, &surface, origin, 1.0, 1.0, Kind::Unspecified);
    let cut =
        whole.into_iter().filter_map(|piece| CropRenderElement::from_element(piece, 1.0, shows));
    pieces.extend(cut.map(Piece::Window));
    Ok(())
}
