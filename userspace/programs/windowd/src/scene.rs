// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a screen's frame is drawn from.
//!
//! From the front: the box windowd shows over the windows, then each
//! window in the windows' order, and of each window its popups, the
//! strips of its frame and its own surface. Under a window that is
//! dragged to an edge of the desktop lies the preview of what it will
//! fill there (ADR 0727). The surface is cut to the
//! places the frame's layout gives it, so that a program draws nowhere
//! else: the content, and the region of a title bar the program shares
//! (ADR 0720). It lies behind the frame, which is see-through there.
//!
//! A surface is one piece of the frame however many parts it shows in:
//! the damage tracker knows a piece by its surface, and two pieces of
//! one surface would each be taken for the other, moved. So a surface
//! is cut to the rectangle around its parts ([`Cut`]), and drawn in the
//! parts alone.

use smithay::backend::renderer::Color32F;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::utils::CropRenderElement;
use smithay::backend::renderer::element::{
    Element, Id, Kind, RenderElement, UnderlyingStorage, render_elements,
};
use smithay::backend::renderer::pixman::{PixmanError, PixmanFrame, PixmanRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::desktop::{PopupManager, Window};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Scale, Transform};
use smithay::wayland::seat::WaylandFocus;

use window_frame::{Colour, Shape, Theme};

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;

use crate::windows::zones::Fills;
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
    /// A rectangle of one colour: the ground under a window's content,
    /// or a part of a preview.
    Solid=SolidColorRenderElement,
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

/// How far in from the edges of the area it stands for a preview's
/// own edges are, and how wide the line around it is, in points.
const PREVIEW_INSET: i32 = 8;
const PREVIEW_LINE: i32 = 2;

/// How much of what is under a preview its inside covers, of 255.
const PREVIEW_COVER: u8 = 110;

/// What the renderer knows the parts of the preview by: its inside and
/// the four lines around it.
#[derive(Debug)]
pub struct Shade {
    parts: [Id; 5],
}

impl Default for Shade {
    fn default() -> Shade {
        Shade { parts: core::array::from_fn(|_| Id::new()) }
    }
}

/// The preview of the area a window that is dragged will fill.
#[derive(Debug, Clone, Copy)]
pub struct Preview<'a> {
    /// The window, which the preview lies under.
    pub window: &'a Window,
    /// The area.
    pub area: Rectangle<i32, Logical>,
    /// What the renderer knows the preview by.
    pub shade: &'a Shade,
}

impl Preview<'_> {
    /// Adds the preview's parts to `pieces`, for the screen `onto`: a
    /// see-through inside with a line around it, in the colours of a
    /// panel's chosen row.
    fn pieces(&self, theme: &Theme, onto: Onto, pieces: &mut Vec<Piece>) {
        let all = Rectangle::new(
            self.area.loc + Point::from((PREVIEW_INSET, PREVIEW_INSET)),
            (self.area.size.w - 2 * PREVIEW_INSET, self.area.size.h - 2 * PREVIEW_INSET).into(),
        );
        if all.size.w <= 2 * PREVIEW_LINE || all.size.h <= 2 * PREVIEW_LINE {
            return;
        }
        let (line, rows) = (PREVIEW_LINE, all.size.h - 2 * PREVIEW_LINE);
        let part = |x: i32, y: i32, w: i32, h: i32| {
            Rectangle::new(all.loc + Point::from((x, y)), (w, h).into())
        };
        let lines = [
            part(0, 0, all.size.w, line),
            part(0, all.size.h - line, all.size.w, line),
            part(0, line, line, rows),
            part(all.size.w - line, line, line, rows),
        ];
        let inside = part(line, line, all.size.w - 2 * line, rows);
        let parts = lines
            .into_iter()
            .map(|line| (line, theme.panel.chosen_line))
            .chain([(inside, theme.panel.chosen.covering(PREVIEW_COVER))]);
        for (id, (rect, colour)) in self.shade.parts.iter().zip(parts) {
            pieces.push(Piece::Solid(SolidColorRenderElement::new(
                id.clone(),
                onto.rect(rect),
                CommitCounter::default(),
                solid(colour),
                Kind::Unspecified,
            )));
        }
    }
}

/// `colour` as the renderer takes one: each of red, green and blue
/// already times how much the colour covers.
fn solid(colour: Colour) -> Color32F {
    let cover = f32::from(colour.cover) / 255.0;
    let part = |part: u8| f32::from(part) / 255.0 * cover;
    Color32F::new(part(colour.red), part(colour.green), part(colour.blue), cover)
}

/// A box of windowd's own over the windows: a panel or the picker.
#[derive(Debug, Clone, Copy)]
pub struct Over<'a> {
    /// Where its top left corner is on the desktop.
    pub at: Point<i32, Logical>,
    /// Its pixels.
    pub pixels: &'a MemoryRenderBuffer,
}

/// What the screens show beside the windows.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// The windows.
    pub windows: &'a Windows,
    /// The box over them.
    pub over: Option<Over<'a>>,
    /// The preview under one of them.
    pub preview: Option<Preview<'a>>,
}

/// A screen as a frame is drawn for it: where the top left corner of
/// its area of the desktop is, and how many of its pixels a point of
/// the desktop is across.
#[derive(Clone, Copy)]
struct Onto {
    corner: Point<i32, Logical>,
    zoom: f64,
}

impl Onto {
    /// The place of the desktop `at` as a place of the frame, to the
    /// nearest pixel.
    fn place(self, at: Point<i32, Logical>) -> Point<i32, Physical> {
        (at - self.corner).to_physical_precise_round(self.zoom)
    }

    /// The rectangle of the desktop `rect` as one of the frame, each
    /// of its edges to the nearest pixel, so that two that share an
    /// edge on the desktop share it in the frame.
    fn rect(self, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Physical> {
        let (from, to) = (self.place(rect.loc), self.place(rect.loc + rect.size.to_point()));
        Rectangle::new(from, (to - from).to_size())
    }
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
        zoom: f64,
    ) -> Result<Vec<Piece>, PixmanError> {
        let onto = Onto { corner: screen.loc, zoom };
        let mut pieces = Vec::new();
        if let Some(over) = self.over {
            let at = onto.place(over.at).to_f64();
            let own = MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                at,
                over.pixels,
                None,
                None,
                None,
                Kind::Unspecified,
            )?;
            pieces.push(Piece::Own(own));
        }
        let theme = self.windows.theme();
        for framed in self.windows.stacked() {
            // A window that fills a part of its screen is on that
            // screen alone: the line and the shadow around its frame
            // do not fall on the screen beside it.
            let frame = framed.fills() == Fills::Nothing || framed.frame().overlaps(screen);
            window(renderer, framed, onto, frame.then_some(theme.content_line), &mut pieces)?;
            if let Some(preview) = self.preview.filter(|preview| *preview.window == framed.window) {
                preview.pieces(theme, onto, &mut pieces);
            }
        }
        Ok(pieces)
    }
}

impl Scene<'_> {
    /// The surface that is all the screen whose area of the desktop is
    /// `screen` shows, if one surface is: the only surface of a window
    /// that fills the screen with no frame, with no popup, in front of
    /// every other window that reaches the screen, and with nothing of
    /// windowd's over it. A frame of that screen is that surface's
    /// pixels and nothing else, if they are as many as the screen's.
    pub fn alone(&self, screen: Rectangle<i32, Logical>) -> Option<WlSurface> {
        if self.over.is_some() || self.preview.is_some() {
            return None;
        }
        for framed in self.windows.stacked() {
            let surface = framed.window.wl_surface()?;
            // A popup may reach anywhere, onto a screen its window is
            // not on too.
            if PopupManager::popups_for_surface(&surface).next().is_some() {
                return None;
            }
            if !framed.frame().overlaps(screen) {
                continue;
            }
            let fills = framed.layout().shape == Shape::Full
                && framed.surface_at() == screen.loc
                && framed.frame() == screen;
            let mut surfaces = 0;
            framed.window.with_surfaces(|_, _| surfaces += 1);
            return (fills && surfaces == 1).then(|| surface.into_owned());
        }
        None
    }
}

/// Adds to `pieces` what `framed` is drawn from on the screen `onto`:
/// its popups, its own pixels, and its frame if `line` is the colour
/// of the frame's line around a window's content.
fn window(
    renderer: &mut PixmanRenderer,
    framed: &Framed,
    onto: Onto,
    line: Option<Colour>,
    pieces: &mut Vec<Piece>,
) -> Result<(), PixmanError> {
    let Some(surface) = framed.window.wl_surface() else { return Ok(()) };
    let origin = onto.place(framed.surface_at());
    for (popup, within) in PopupManager::popups_for_surface(&surface) {
        let offset = framed.window.geometry().loc + within - popup.geometry().loc;
        pieces.extend(render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            onto.place(framed.surface_at() + offset),
            onto.zoom,
            1.0,
            Kind::Unspecified,
        ));
    }
    for strip in framed.strips().iter().filter(|_| line.is_some()) {
        let at = framed.at() + Point::from((strip.rect.x, strip.rect.y));
        let own = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            onto.place(at).to_f64(),
            &strip.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )?;
        pieces.push(Piece::Own(own));
    }
    let mut parts = framed.shows().map(|part| onto.rect(part));
    let parts = [parts.next(), parts.next()];
    let whole: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> = render_elements_from_surface_tree(
        renderer,
        &surface,
        origin,
        onto.zoom,
        1.0,
        Kind::Unspecified,
    );
    let cut = whole.into_iter().filter_map(|surface| Cut::of(surface, parts));
    pieces.extend(cut.map(Piece::Window));
    if let Some((id, around)) = framed.around() {
        pieces.push(Piece::Solid(SolidColorRenderElement::new(
            id.clone(),
            onto.rect(around),
            CommitCounter::default(),
            solid(Colour::BLACK),
            Kind::Unspecified,
        )));
    } else if let Some(line) = line
        && onto.zoom.fract() != 0.0
        && framed.layout().bar_rows() > 0
    {
        let (id, ground) = framed.ground();
        pieces.push(Piece::Solid(SolidColorRenderElement::new(
            id.clone(),
            onto.rect(ground),
            CommitCounter::default(),
            solid(line),
            Kind::Unspecified,
        )));
    }
    Ok(())
}
