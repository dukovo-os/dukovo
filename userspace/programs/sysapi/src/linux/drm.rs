// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's interface to a graphics card's screens (DRM and its mode
//! setting, drm.h and drm_mode.h), as far as the Linux side of the
//! display benchmarks uses it (`tools/linux-bench`): a picture of plain
//! memory put on a screen, a change of the picture checked or made, a
//! part of it named as new, and the pointer moved.
//!
//! Each request is a record the kernel reads and fills. Where a record
//! names an array, the array is a `Vec` or a value this module holds
//! through the call, of the length the record gives, and a length of 0
//! has the kernel write none. The requests the benchmarks time are
//! inlined into them.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::checked;

/// The number of a DRM request that the kernel reads a `T` of, and
/// writes one when `back` (asm-generic/ioctl.h's _IOW and _IOWR, of the
/// type 'd').
const fn number<T>(nr: usize, back: bool) -> libc::c_ulong {
    let direction = if back { 3 } else { 1 };
    ((direction << 30) | (size_of::<T>() << 16) | (0x64 << 8) | nr) as libc::c_ulong
}

const VERSION: libc::c_ulong = number::<Version>(0x00, true);
const SET_CLIENT_CAP: libc::c_ulong = number::<ClientCap>(0x0d, false);
const GETRESOURCES: libc::c_ulong = number::<CardRes>(0xa0, true);
const SETCRTC: libc::c_ulong = number::<Crtc>(0xa2, true);
const CURSOR: libc::c_ulong = number::<Cursor>(0xa3, true);
const GETCONNECTOR: libc::c_ulong = number::<GetConnector>(0xa7, true);
const GETPROPERTY: libc::c_ulong = number::<GetProperty>(0xaa, true);
const ADDFB: libc::c_ulong = number::<FbCmd>(0xae, true);
const DIRTYFB: libc::c_ulong = number::<FbDirty>(0xb1, true);
const CREATE_DUMB: libc::c_ulong = number::<CreateDumb>(0xb2, true);
const GETPLANERESOURCES: libc::c_ulong = number::<PlaneRes>(0xb5, true);
const GETPLANE: libc::c_ulong = number::<GetPlane>(0xb6, true);
const OBJ_GETPROPERTIES: libc::c_ulong = number::<ObjProperties>(0xb9, true);
const ATOMIC: libc::c_ulong = number::<Atomic>(0xbc, true);

/// DRM_CLIENT_CAP_ATOMIC: the program makes its changes as one checked
/// set, and is shown every plane and the properties such a change sets.
const CLIENT_CAP_ATOMIC: u64 = 3;

/// DRM_MODE_OBJECT_PLANE: the kind of thing a plane is.
const OBJECT_PLANE: u32 = 0xeeee_eeee;

/// A connector's `connection` when a screen is plugged into it.
const CONNECTED: u32 = 1;

/// [`Cursor::flags`]: the request gives the pointer its picture
/// (DRM_MODE_CURSOR_BO), or moves it (DRM_MODE_CURSOR_MOVE).
const CURSOR_BO: u32 = 1;
const CURSOR_MOVE: u32 = 2;

/// [`Card::commit`]'s flag for a change that is checked and not made
/// (DRM_MODE_ATOMIC_TEST_ONLY).
pub const TEST_ONLY: u32 = 0x0100;

/// The value of a plane's `type` property for a screen's main plane.
pub const PLANE_PRIMARY: u64 = 1;

/// drm_version.
#[repr(C)]
#[derive(Default)]
struct Version {
    major: i32,
    minor: i32,
    patch: i32,
    pad: u32,
    name_len: usize,
    name: usize,
    date_len: usize,
    date: usize,
    desc_len: usize,
    desc: usize,
}

/// drm_set_client_cap.
#[repr(C)]
struct ClientCap {
    capability: u64,
    value: u64,
}

/// drm_mode_card_res.
#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

/// One way a screen can be driven (drm_mode_modeinfo), kept as the
/// kernel gave it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mode {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}

impl Mode {
    /// The pixels across and down that the mode shows.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (u32::from(self.hdisplay), u32::from(self.vdisplay))
    }
}

/// drm_mode_crtc.
#[repr(C)]
#[derive(Default)]
struct Crtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: Mode,
}

/// drm_mode_cursor.
#[repr(C)]
#[derive(Default)]
struct Cursor {
    flags: u32,
    crtc_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    handle: u32,
}

/// drm_mode_get_connector.
#[repr(C)]
#[derive(Default)]
struct GetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

/// drm_mode_get_property.
#[repr(C)]
#[derive(Default)]
struct GetProperty {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

/// drm_mode_fb_cmd.
#[repr(C)]
#[derive(Default)]
struct FbCmd {
    fb_id: u32,
    width: u32,
    height: u32,
    pitch: u32,
    bpp: u32,
    depth: u32,
    handle: u32,
}

/// drm_mode_fb_dirty_cmd.
#[repr(C)]
#[derive(Default)]
struct FbDirty {
    fb_id: u32,
    flags: u32,
    color: u32,
    num_clips: u32,
    clips_ptr: u64,
}

/// drm_clip_rect: a rectangle from its first column and row to the ones
/// past its last.
#[repr(C)]
struct Clip {
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
}

/// drm_mode_create_dumb.
#[repr(C)]
#[derive(Default)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

/// drm_mode_get_plane_res.
#[repr(C)]
#[derive(Default)]
struct PlaneRes {
    plane_id_ptr: u64,
    count_planes: u32,
    pad: u32,
}

/// drm_mode_get_plane.
#[repr(C)]
#[derive(Default)]
struct GetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

/// drm_mode_obj_get_properties.
#[repr(C)]
#[derive(Default)]
struct ObjProperties {
    props_ptr: u64,
    prop_values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
    pad: u32,
}

/// drm_mode_atomic.
#[repr(C)]
#[derive(Default)]
struct Atomic {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

/// Where the kernel finds `values`, as a record names an array.
fn place<T>(values: &mut [T]) -> u64 {
    values.as_mut_ptr() as u64
}

/// A card's CRTCs, each of which drives a screen, and its connectors.
#[derive(Debug)]
pub struct Resources {
    /// Its CRTCs.
    pub crtcs: Vec<u32>,
    /// Its connectors, with or without a screen plugged in.
    pub connectors: Vec<u32>,
}

/// A picture of plain memory the card made (a "dumb buffer"), 32 bits a
/// pixel, all zeros until it is drawn into.
#[derive(Clone, Copy, Debug)]
pub struct Picture {
    handle: u32,
    pitch: u32,
    width: u32,
    height: u32,
}

/// A graphics card's file, _/dev/dri/cardN_. The first program to open
/// one is the card's master, which may change what its screens show.
#[derive(Debug)]
pub struct Card {
    file: File,
}

impl Card {
    /// Opens the card at `path`, and asks to make changes as checked
    /// sets (DRM_CLIENT_CAP_ATOMIC).
    ///
    /// # Errors
    ///
    /// ENOENT for no such card, EOPNOTSUPP for a driver with no such
    /// changes.
    pub fn open(path: &Path) -> io::Result<Card> {
        let card = Card { file: File::options().read(true).write(true).open(path)? };
        let mut cap = ClientCap { capability: CLIENT_CAP_ATOMIC, value: 1 };
        // SAFETY: the record names no array.
        unsafe { card.request(SET_CLIENT_CAP, &mut cap) }?;
        Ok(card)
    }

    /// Makes the request `number` with `record`, again when a signal
    /// broke it off.
    ///
    /// # Safety
    ///
    /// `record` is the record of the request `number`, and each array it
    /// names lives through the call and is as long as the record says.
    #[inline]
    unsafe fn request<T>(&self, number: libc::c_ulong, record: &mut T) -> io::Result<()> {
        loop {
            // SAFETY: the record is whole and lives through the call, and
            // the caller answers for what it names.
            let done =
                checked(unsafe { libc::ioctl(self.file.as_raw_fd(), number, &raw mut *record) });
            match done {
                Ok(_) => return Ok(()),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
    }

    /// The name of the card's driver, as `virtio_gpu`.
    ///
    /// # Errors
    ///
    /// ENOTTY for a file that is no card.
    pub fn driver(&self) -> io::Result<String> {
        let mut name = vec![0u8; 64];
        let mut version = Version {
            name_len: name.len(),
            name: name.as_mut_ptr() as usize,
            ..Version::default()
        };
        // SAFETY: the name's array is `name`, of the length given; the
        // date and the description are given no room.
        unsafe { self.request(VERSION, &mut version) }?;
        name.truncate(version.name_len.min(name.len()));
        Ok(String::from_utf8_lossy(&name).into_owned())
    }

    /// The card's CRTCs and connectors.
    ///
    /// # Errors
    ///
    /// EOPNOTSUPP for a card that sets no modes.
    pub fn resources(&self) -> io::Result<Resources> {
        let mut counts = CardRes::default();
        // SAFETY: every count is 0.
        unsafe { self.request(GETRESOURCES, &mut counts) }?;
        let mut crtcs = vec![0u32; counts.count_crtcs as usize];
        let mut connectors = vec![0u32; counts.count_connectors as usize];
        let mut filled = CardRes {
            crtc_id_ptr: place(&mut crtcs),
            connector_id_ptr: place(&mut connectors),
            count_crtcs: counts.count_crtcs,
            count_connectors: counts.count_connectors,
            ..CardRes::default()
        };
        // SAFETY: the two arrays are as long as their counts, and the
        // counts of framebuffers and encoders are 0.
        unsafe { self.request(GETRESOURCES, &mut filled) }?;
        crtcs.truncate(filled.count_crtcs as usize);
        connectors.truncate(filled.count_connectors as usize);
        Ok(Resources { crtcs, connectors })
    }

    /// The modes of the screen plugged into `connector`, the one it
    /// prefers first; none when nothing is plugged in.
    ///
    /// # Errors
    ///
    /// ENOENT for no such connector.
    pub fn modes(&self, connector: u32) -> io::Result<Vec<Mode>> {
        let mut counts = GetConnector { connector_id: connector, ..GetConnector::default() };
        // SAFETY: every count is 0. With no room for modes the kernel
        // asks the screen afresh.
        unsafe { self.request(GETCONNECTOR, &mut counts) }?;
        if counts.connection != CONNECTED {
            return Ok(Vec::new());
        }
        let mut modes = vec![Mode::default(); counts.count_modes as usize];
        let mut filled = GetConnector {
            connector_id: connector,
            modes_ptr: place(&mut modes),
            count_modes: counts.count_modes,
            ..GetConnector::default()
        };
        // SAFETY: the modes' array is as long as its count, and the
        // counts of encoders and properties are 0.
        unsafe { self.request(GETCONNECTOR, &mut filled) }?;
        modes.truncate(filled.count_modes as usize);
        Ok(modes)
    }

    /// The card's planes: main planes, pointers and overlays.
    ///
    /// # Errors
    ///
    /// EOPNOTSUPP for a card that sets no modes.
    pub fn planes(&self) -> io::Result<Vec<u32>> {
        let mut counts = PlaneRes::default();
        // SAFETY: the count is 0.
        unsafe { self.request(GETPLANERESOURCES, &mut counts) }?;
        let mut planes = vec![0u32; counts.count_planes as usize];
        let mut filled = PlaneRes {
            plane_id_ptr: place(&mut planes),
            count_planes: counts.count_planes,
            ..PlaneRes::default()
        };
        // SAFETY: the array is as long as its count.
        unsafe { self.request(GETPLANERESOURCES, &mut filled) }?;
        planes.truncate(filled.count_planes as usize);
        Ok(planes)
    }

    /// The CRTC that `plane` is shown on now; 0 for none.
    ///
    /// # Errors
    ///
    /// ENOENT for no such plane.
    pub fn plane_crtc(&self, plane: u32) -> io::Result<u32> {
        let mut record = GetPlane { plane_id: plane, ..GetPlane::default() };
        // SAFETY: the count of formats is 0.
        unsafe { self.request(GETPLANE, &mut record) }?;
        Ok(record.crtc_id)
    }

    /// The properties of `plane`, each with its name and its value now.
    ///
    /// # Errors
    ///
    /// ENOENT for no such plane.
    pub fn plane_properties(&self, plane: u32) -> io::Result<Vec<(u32, String, u64)>> {
        let of = |count| ObjProperties {
            count_props: count,
            obj_id: plane,
            obj_type: OBJECT_PLANE,
            ..ObjProperties::default()
        };
        let mut counts = of(0);
        // SAFETY: the count is 0.
        unsafe { self.request(OBJ_GETPROPERTIES, &mut counts) }?;
        let mut ids = vec![0u32; counts.count_props as usize];
        let mut values = vec![0u64; counts.count_props as usize];
        let mut filled = of(counts.count_props);
        filled.props_ptr = place(&mut ids);
        filled.prop_values_ptr = place(&mut values);
        // SAFETY: the two arrays are as long as the count.
        unsafe { self.request(OBJ_GETPROPERTIES, &mut filled) }?;
        ids.truncate(filled.count_props as usize);
        ids.iter()
            .zip(values)
            .map(|(&id, value)| Ok((id, self.property_name(id)?, value)))
            .collect()
    }

    /// The name of the property `property`, as `FB_ID`.
    fn property_name(&self, property: u32) -> io::Result<String> {
        let mut record = GetProperty { prop_id: property, ..GetProperty::default() };
        // SAFETY: the counts of values and of named values are 0.
        unsafe { self.request(GETPROPERTY, &mut record) }?;
        let len = record.name.iter().position(|&byte| byte == 0).unwrap_or(record.name.len());
        Ok(String::from_utf8_lossy(&record.name[..len]).into_owned())
    }

    /// A new picture of `width` by `height` pixels.
    ///
    /// # Errors
    ///
    /// ENOMEM, or EINVAL for a size the card cannot hold.
    pub fn picture(&self, width: u32, height: u32) -> io::Result<Picture> {
        let mut record = CreateDumb { width, height, bpp: 32, ..CreateDumb::default() };
        // SAFETY: the record names no array.
        unsafe { self.request(CREATE_DUMB, &mut record) }?;
        Ok(Picture { handle: record.handle, pitch: record.pitch, width, height })
    }

    /// A framebuffer of `picture`, which a plane can show: its pixels
    /// are XRGB, eight bits each.
    ///
    /// # Errors
    ///
    /// EINVAL for a picture the card cannot show.
    pub fn framebuffer(&self, picture: &Picture) -> io::Result<u32> {
        let mut record = FbCmd {
            width: picture.width,
            height: picture.height,
            pitch: picture.pitch,
            bpp: 32,
            depth: 24,
            handle: picture.handle,
            ..FbCmd::default()
        };
        // SAFETY: the record names no array.
        unsafe { self.request(ADDFB, &mut record) }?;
        Ok(record.fb_id)
    }

    /// Has `crtc` drive the screen at `connector` in `mode`, showing the
    /// framebuffer `framebuffer`.
    ///
    /// # Errors
    ///
    /// EACCES for a program that is not the card's master, EINVAL for a
    /// mode the screen or the framebuffer does not fit.
    pub fn set_mode(
        &self,
        crtc: u32,
        connector: u32,
        mode: &Mode,
        framebuffer: u32,
    ) -> io::Result<()> {
        let mut connectors = [connector];
        let mut record = Crtc {
            set_connectors_ptr: place(&mut connectors),
            count_connectors: 1,
            crtc_id: crtc,
            fb_id: framebuffer,
            mode_valid: 1,
            mode: *mode,
            ..Crtc::default()
        };
        // SAFETY: the connectors' array holds the one the count gives.
        unsafe { self.request(SETCRTC, &mut record) }
    }

    /// One change: `property` of `plane` becomes `value`. With `flags`
    /// 0 the change is made and the call returns when it has been;
    /// with [`TEST_ONLY`] it is checked and nothing changes.
    ///
    /// # Errors
    ///
    /// EINVAL for a change the card cannot make, EBUSY while one made
    /// before has not shown.
    #[inline]
    pub fn commit(&self, flags: u32, plane: u32, property: u32, value: u64) -> io::Result<()> {
        let (mut planes, mut counts, mut properties, mut values) =
            ([plane], [1u32], [property], [value]);
        let mut record = Atomic {
            flags,
            count_objs: 1,
            objs_ptr: place(&mut planes),
            count_props_ptr: place(&mut counts),
            props_ptr: place(&mut properties),
            prop_values_ptr: place(&mut values),
            ..Atomic::default()
        };
        // SAFETY: one thing with one property: each array holds one.
        unsafe { self.request(ATOMIC, &mut record) }
    }

    /// Names the `side` by `side` pixels at (`x`, `y`) of `framebuffer`
    /// as drawn anew, for the card to show them.
    ///
    /// # Errors
    ///
    /// ENOENT for no such framebuffer.
    #[inline]
    pub fn changed(&self, framebuffer: u32, x: u16, y: u16, side: u16) -> io::Result<()> {
        let mut clips = [Clip { x1: x, y1: y, x2: x + side, y2: y + side }];
        let mut record = FbDirty {
            fb_id: framebuffer,
            num_clips: 1,
            clips_ptr: place(&mut clips),
            ..FbDirty::default()
        };
        // SAFETY: the rectangles' array holds the one the count gives.
        unsafe { self.request(DIRTYFB, &mut record) }
    }

    /// Gives the pointer of `crtc`'s screen the picture `picture`.
    ///
    /// # Errors
    ///
    /// ENXIO for a card with no pointer plane, EINVAL for a picture of
    /// a size its pointer cannot be.
    pub fn set_pointer(&self, crtc: u32, picture: &Picture) -> io::Result<()> {
        let mut record = Cursor {
            flags: CURSOR_BO,
            crtc_id: crtc,
            width: picture.width,
            height: picture.height,
            handle: picture.handle,
            ..Cursor::default()
        };
        // SAFETY: the record names no array.
        unsafe { self.request(CURSOR, &mut record) }
    }

    /// Moves the pointer of `crtc`'s screen to (`x`, `y`).
    ///
    /// # Errors
    ///
    /// ENXIO for a card with no pointer plane.
    #[inline]
    pub fn move_pointer(&self, crtc: u32, x: i32, y: i32) -> io::Result<()> {
        let mut record = Cursor { flags: CURSOR_MOVE, crtc_id: crtc, x, y, ..Cursor::default() };
        // SAFETY: the record names no array.
        unsafe { self.request(CURSOR, &mut record) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers drm.h and drm_mode.h give these requests on x86-64:
    /// each holds its record's size, so a record laid out wrongly shows
    /// here.
    #[test]
    fn the_requests_have_the_kernels_numbers() {
        assert_eq!(VERSION, 0xc040_6400);
        assert_eq!(SET_CLIENT_CAP, 0x4010_640d);
        assert_eq!(GETRESOURCES, 0xc040_64a0);
        assert_eq!(SETCRTC, 0xc068_64a2);
        assert_eq!(CURSOR, 0xc01c_64a3);
        assert_eq!(GETCONNECTOR, 0xc050_64a7);
        assert_eq!(GETPROPERTY, 0xc040_64aa);
        assert_eq!(ADDFB, 0xc01c_64ae);
        assert_eq!(DIRTYFB, 0xc018_64b1);
        assert_eq!(CREATE_DUMB, 0xc020_64b2);
        assert_eq!(GETPLANERESOURCES, 0xc010_64b5);
        assert_eq!(GETPLANE, 0xc020_64b6);
        assert_eq!(OBJ_GETPROPERTIES, 0xc020_64b9);
        assert_eq!(ATOMIC, 0xc038_64bc);
        assert_eq!(size_of::<Mode>(), 68);
        assert_eq!(size_of::<Clip>(), 8);
    }

    #[test]
    fn a_file_that_is_no_card_is_refused() {
        let err = Card::open(Path::new("/dev/null")).unwrap_err();
        assert!(matches!(err.raw_os_error(), Some(libc::ENOTTY | libc::EINVAL)), "{err}");
    }
}
