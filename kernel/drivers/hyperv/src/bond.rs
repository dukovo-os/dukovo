// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A synthetic network card and its virtual function, as one card:
//! Azure's accelerated networking (ADR 0425).
//!
//! The host can give a synthetic card ([`Netvsc`]) a virtual function of
//! a real card: a PCI function passed through to the guest on a virtual
//! PCI bus ([`crate::pci`]), with the same MAC address, whose frames skip
//! the host's virtual switch. The host tells the synthetic card that it
//! has one (the VF association), the guest asks for the frames to take
//! the function's path (SET_DATAPATH), and the host can take the function
//! away at any time: for maintenance, or before it moves the machine.
//! The stack sees one card, a [`Bond`], with the synthetic card's MAC
//! address and link.
//!
//! The function is set up elsewhere (on VMBus's thread, which may wait)
//! and handed to the bond through a [`Handoff`]. The bond takes it once
//! the host has associated a function with the synthetic card, sends
//! through it while it works, and receives from both cards in turn: the
//! host still sends some frames through the synthetic card, as while the
//! path changes. Asked for the function back, the bond turns the path
//! back to the synthetic card and gives the function back as it is: the
//! thread that set it up stops it, which waits for the card.

use radix_nic::{Info, Link, Nic};

use crate::network::Netvsc;

/// A synthetic card that can have a virtual function: [`Netvsc`].
pub trait Synthetic: Nic {
    /// The serial number of the virtual function the host associates
    /// with the card, if it associates one.
    fn virtual_function(&self) -> Option<u32>;
    /// Asks the host to send the card's frames through its virtual
    /// function (`vf`), or through the card itself. Returns whether it
    /// was asked.
    fn set_datapath(&mut self, vf: bool) -> bool;
}

impl Synthetic for Netvsc {
    fn virtual_function(&self) -> Option<u32> {
        Netvsc::virtual_function(self)
    }

    fn set_datapath(&mut self, vf: bool) -> bool {
        Netvsc::set_datapath(self, vf)
    }
}

/// Where a bond's virtual function comes from and goes back to: the
/// kernel's, for the one synthetic card the bond holds.
pub trait Handoff<F>: Send {
    /// The virtual function offered for the card, if one waits: it is
    /// the bond's until it gives it back.
    fn take(&mut self) -> Option<F>;
    /// Whether the virtual function the bond has is asked back.
    fn asked_back(&self) -> bool;
    /// Gives the virtual function back, still running.
    fn give_back(&mut self, vf: F);
}

/// A synthetic card and the virtual function it may have, as one card.
pub struct Bond<S, F, H> {
    synthetic: S,
    vf: Option<F>,
    handoff: H,
    /// Whether the next frame is looked for at the virtual function
    /// first: the two cards take turns, so that neither waits behind the
    /// other.
    vf_first: bool,
    /// Whether the path was turned back to the synthetic card because
    /// the virtual function failed.
    vf_failed: bool,
}

impl<S: Synthetic, F: Nic, H: Handoff<F>> Bond<S, F, H> {
    /// The synthetic card `synthetic`, which takes its virtual functions
    /// from `handoff`.
    pub fn new(synthetic: S, handoff: H) -> Bond<S, F, H> {
        Bond { synthetic, vf: None, handoff, vf_first: false, vf_failed: false }
    }

    /// The synthetic card.
    pub fn synthetic(&self) -> &S {
        &self.synthetic
    }

    /// Whether the bond has a virtual function now.
    pub fn has_vf(&self) -> bool {
        self.vf.is_some()
    }

    /// Whether frames are sent through the virtual function: while the
    /// bond has one that works and the host associates one.
    pub fn sends_through_vf(&self) -> bool {
        self.vf.as_ref().is_some_and(|vf| !vf.failed())
            && self.synthetic.virtual_function().is_some()
    }

    /// Gives the virtual function back if it is asked back, takes one if
    /// it is offered, and turns the path back to the synthetic card if
    /// the one it has failed.
    fn look(&mut self) {
        if self.vf.is_some() && self.handoff.asked_back() {
            self.synthetic.set_datapath(false);
            if let Some(vf) = self.vf.take() {
                self.handoff.give_back(vf);
            }
            self.vf_failed = false;
            return;
        }
        if self.vf.is_none() && self.synthetic.virtual_function().is_some() {
            if let Some(vf) = self.handoff.take() {
                self.vf = Some(vf);
                self.vf_failed = false;
                self.synthetic.set_datapath(true);
            }
            return;
        }
        if !self.vf_failed && self.vf.as_ref().is_some_and(Nic::failed) {
            self.vf_failed = true;
            self.synthetic.set_datapath(false);
        }
    }

    /// The next frame the virtual function received, if it has one and
    /// it works.
    fn receive_vf(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.vf.as_mut().filter(|vf| !vf.failed())?.receive(frame)
    }
}

impl<S: Synthetic, F: Nic, H: Handoff<F>> Nic for Bond<S, F, H> {
    fn info(&self) -> Info {
        self.synthetic.info()
    }

    fn link(&self) -> Link {
        self.synthetic.link()
    }

    fn failed(&self) -> bool {
        self.synthetic.failed()
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.look();
        self.vf_first = !self.vf_first;
        if self.vf_first {
            match self.receive_vf(frame) {
                Some(len) => Some(len),
                None => self.synthetic.receive(frame),
            }
        } else {
            match self.synthetic.receive(frame) {
                Some(len) => Some(len),
                None => self.receive_vf(frame),
            }
        }
    }

    fn can_send(&mut self) -> bool {
        // The synthetic card takes its sent frames' slots back either way.
        let synthetic = self.synthetic.can_send();
        if self.sends_through_vf() {
            self.vf.as_mut().is_some_and(Nic::can_send)
        } else {
            synthetic
        }
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        if self.sends_through_vf()
            && let Some(vf) = self.vf.as_mut()
        {
            return vf.send(frame);
        }
        self.synthetic.send(frame)
    }

    fn flush(&mut self) {
        self.look();
        self.synthetic.flush();
        if let Some(vf) = self.vf.as_mut() {
            vf.flush();
        }
    }

    fn dropped(&self) -> u64 {
        let vf = self.vf.as_ref().map_or(0, Nic::dropped);
        self.synthetic.dropped() + vf
    }

    fn stop(&mut self) {
        // The virtual function goes back first, so that the thread that
        // set it up can stop it and tell the host.
        if let Some(vf) = self.vf.take() {
            self.synthetic.set_datapath(false);
            self.handoff.give_back(vf);
        }
        self.synthetic.stop();
    }
}
