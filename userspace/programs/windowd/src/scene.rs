// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a screen's frame is drawn from.
//!
//! From the front: the panel windowd shows over the windows, then each
//! window in the windows' order, and of each window its popups, the
//! strips of its frame and its own surface. The surface is cut to the
//! places the frame's layout gives it, so that a program draws nowhere
//! else: the content, and the region of a title bar the program shares
//! (ADR 0719). It lies behind the frame, which is see-through there.
//!
//! A surface is one piece of the frame however many parts it shows in:
//! the damage tracker knows a piece by its surface, and two pieces of
//! one surface would each be taken for the other, moved. So a surface
//! is cut to the rectangle around its parts ([`Cut`]), and drawn in the
//! parts alone.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::{
    Element, Id, Kind, RenderElement, UnderlyingStorage, render_elements,
};
use smithay::backend::renderer::pixman::{PixmanError, PixmanFrame, PixmanRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::desktop::PopupManager;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform};
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
    /// A surface of a window, where the window's frame lets it show.
    Window=Cut,
}

/// A surface of a window, which shows in some parts of a screen's frame
/// and nowhere else.
#[derive(Debug)]
pub struct Cut {
    /// The surface, cut to the rectangle around the parts.
    around: CropRenderElement<WaylandSurfaceRenderElement<PixmanRenderer>>,
    /// The parts.
    parts: [Option<Rectangle<i32, Physical>>; 2],
}

impl Cut {
    /// `surface` where it lies in `parts` of a screen's frame, if
    /// anywhere.
    fn of(
        surface: WaylandSurfaceRenderElement<PixmanRenderer>,
        parts: [Option<Rectangle<i32, Physical>>; 2],
    ) -> Option<Cut> {
        let around = parts.into_iter().flatten().reduce(Rectangle::merge)?;
        let around = CropRenderElement::from_element(surface, 1.0, around)?;
        Some(Cut { around, parts })
    }

    /// What of `rects`, which are reckoned from the place `from` of the
    /// screen's frame, lies in the parts.
    fn kept(
        &self,
        from: Point<i32, Physical>,
        rects: impl IntoIterator<Item = Rectangle<i32, Physical>>,
    ) -> impl Iterator<Item = Rectangle<i32, Physical>> {
        rects.into_iter().flat_map(move |rect| {
            let on_screen = Rectangle::new(rect.loc + from, rect.size);
            self.parts.into_iter().flatten().filter_map(move |part| {
                let kept = on_screen.intersection(part)?;
                Some(Rectangle::new(kept.loc - from, kept.size))
            })
        })
    }
}

impl Element for Cut {
    fn id(&self) -> &Id {
        self.around.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.around.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.around.src()
    }

    fn transform(&self) -> Transform {
        self.around.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.around.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.around.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // Between the parts the surface hides nothing: what is under
        // it there shows wherever the frame over it is see-through.
        let from = self.geometry(scale).loc;
        self.kept(from, self.around.opaque_regions(scale)).collect()
    }

    fn alpha(&self) -> f32 {
        self.around.alpha()
    }

    fn kind(&self) -> Kind {
        self.around.kind()
    }
}

impl RenderElement<PixmanRenderer> for Cut {
    fn draw(
        &self,
        frame: &mut PixmanFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), PixmanError> {
        // The renderer draws in the damage alone.
        let damage: Vec<_> = self.kept(dst.loc, damage.iter().copied()).collect();
        if damage.is_empty() {
            return Ok(());
        }
        self.around.draw(frame, src, dst, &damage, opaque_regions)
    }

    fn underlying_storage(&self, renderer: &mut PixmanRenderer) -> Option<UnderlyingStorage<'_>> {
        self.around.underlying_storage(renderer)
    }
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
    let mut parts = framed
        .shows()
        .map(|part| Rectangle::new(on_screen(part.loc, screen), part.size.to_physical(1)));
    let parts = [parts.next(), parts.next()];
    let whole: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> =
        render_elements_from_surface_tree(renderer, &surface, origin, 1.0, 1.0, Kind::Unspecified);
    let cut = whole.into_iter().filter_map(|surface| Cut::of(surface, parts));
    pieces.extend(cut.map(Piece::Window));
    Ok(())
}
