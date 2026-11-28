// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;

const THUNKS: Thunks = Thunks {
    retpoline: 0xffff_ffff_8010_0000,
    ret: 0xffff_ffff_8010_0060,
    aligned_indirect: 0xffff_ffff_8010_00a0,
};

/// The six bytes of a site at `at` of `form` that leads to `to`.
fn site(at: u64, form: Form, to: u64) -> [u8; SITE] {
    let displacement = to.wrapping_sub(at + SITE as u64) as i64 as i32;
    let head = match form {
        Form::Call => [CS, CALL],
        Form::Jump => [CS, JMP],
        Form::Branch(condition) => [0x0f, 0x80 | condition],
    };
    let mut out = [0; SITE];
    out[..2].copy_from_slice(&head);
    out[2..].copy_from_slice(&displacement.to_le_bytes());
    out
}

fn rewritten(at: u64, form: Form, to: u64, indirect: Indirect, returns: Return) -> [u8; SITE] {
    rewrite(at, site(at, form, to), &THUNKS, indirect, returns).unwrap()
}

#[test]
fn a_table_round_trips_and_a_bad_one_is_refused() {
    let mut buf = vec![0xffu8; 64];
    let len = encode(&[0x10, 0x16, 0x4000], &mut buf).unwrap();
    assert_eq!(len, 44);
    let table = Table::parse(&buf).unwrap();
    assert_eq!(table.len(), 3);
    assert_eq!(table.offsets().collect::<Vec<_>>(), [0x10, 0x16, 0x4000]);
    // Out of order, or two sites that share bytes.
    assert_eq!(encode(&[0x20, 0x10], &mut buf), Err(EncodeError::Unsorted));
    assert_eq!(encode(&[0x10, 0x15], &mut buf), Err(EncodeError::Unsorted));
    assert_eq!(encode(&[0; 9], &mut buf[..40]), Err(EncodeError::TooSmall { needed: 68 }));
    // The zeros of a kernel that was never post-processed.
    assert!(Table::parse(&[0; 64]).is_none());
    buf[16] = 99;
    assert!(Table::parse(&buf).is_none());
}

#[test]
fn a_site_says_what_it_is_by_the_thunk_it_leads_to() {
    let at = 0xffff_ffff_8020_0000;
    let read = |form, to| decode(at, site(at, form, to), &THUNKS);
    assert_eq!(
        read(Form::Call, THUNKS.retpoline),
        Ok(Site { form: Form::Call, kind: Kind::Indirect })
    );
    assert_eq!(read(Form::Jump, THUNKS.ret), Ok(Site { form: Form::Jump, kind: Kind::Return }));
    assert_eq!(
        read(Form::Branch(5), THUNKS.ret),
        Ok(Site { form: Form::Branch(5), kind: Kind::Return })
    );
    // A call to some function, a call to the return thunk, and bytes that
    // are no branch at all: the kernel stops rather than change them.
    let function = at + 0x1234;
    assert_eq!(read(Form::Call, function), Err(SiteError::Elsewhere(function)));
    assert_eq!(read(Form::Call, THUNKS.ret), Err(SiteError::CallToReturn));
    let plain_call = [CALL, 0, 0, 0, 0, 0x90];
    assert_eq!(decode(at, plain_call, &THUNKS), Err(SiteError::NotABranch(plain_call)));
}

#[test]
fn plain_branches_and_returns_replace_the_thunks_where_none_is_needed() {
    let at = 0xffff_ffff_8020_0010;
    let (plain, ret) = (Indirect::Plain, Return::Plain);
    // call r11, then a 3-byte no-op; jmp r11, then traps.
    assert_eq!(
        rewritten(at, Form::Call, THUNKS.retpoline, plain, ret),
        [0x41, 0xff, 0xd3, 0x0f, 0x1f, 0x00]
    );
    assert_eq!(
        rewritten(at, Form::Jump, THUNKS.retpoline, plain, ret),
        [0x41, 0xff, 0xe3, INT3, INT3, INT3]
    );
    // jne r11 is "je over the jump; jmp r11".
    assert_eq!(
        rewritten(at, Form::Branch(5), THUNKS.retpoline, plain, ret),
        [0x74, 3, 0x41, 0xff, 0xe3, INT3]
    );
    assert_eq!(
        rewritten(at, Form::Jump, THUNKS.ret, plain, ret),
        [RET, INT3, INT3, INT3, INT3, INT3]
    );
    // jb ret is "jae over the ret; ret".
    assert_eq!(
        rewritten(at, Form::Branch(2), THUNKS.ret, plain, ret),
        [0x73, 1, RET, INT3, INT3, INT3]
    );
}

#[test]
fn the_retpoline_and_a_return_thunk_keep_their_sites_as_branches() {
    let at = 0xffff_ffff_8020_0010;
    let call = site(at, Form::Call, THUNKS.retpoline);
    assert_eq!(rewrite(at, call, &THUNKS, Indirect::Retpoline, Return::Plain), Ok(call));
    let thunk = 0xffff_ffff_8030_0040;
    for form in [Form::Jump, Form::Branch(4)] {
        let to_thunk = rewritten(at, form, THUNKS.ret, Indirect::Plain, Return::Thunk(thunk));
        assert_eq!(to_thunk[..2], site(at, form, thunk)[..2]);
        assert_eq!(target(at, to_thunk), thunk);
    }
    // A thunk out of reach of a 32-bit displacement is refused.
    let far = at - (3 << 31);
    assert_eq!(
        rewrite(at, site(at, Form::Jump, THUNKS.ret), &THUNKS, Indirect::Plain, Return::Thunk(far)),
        Err(SiteError::OutOfReach(far))
    );
}

/// Every place in a cache line, for each form: an aligned rewrite leaves
/// no indirect branch or return ending in the lower half of a line,
/// keeps the site's own effect, and falls back to a thunk only when no
/// place in the site's six bytes is in the upper half.
#[test]
fn aligned_branches_and_returns_end_in_the_upper_half_of_their_lines() {
    let base = 0xffff_ffff_8040_0000;
    for offset in 0..64 {
        let at = base + offset;
        let fits_upper = |first: usize, room: usize, last_from: usize| {
            (first..=first + room).any(|start| upper_half(at + (start + last_from) as u64))
        };
        for form in [Form::Call, Form::Jump, Form::Branch(3)] {
            let bytes = rewritten(at, form, THUNKS.retpoline, Indirect::Aligned, Return::Plain);
            let first = if matches!(form, Form::Branch(_)) { 2 } else { 0 };
            let to_thunk = bytes == site(at, form, THUNKS.aligned_indirect);
            match bytes.windows(3).position(|w| w == CALL_R11 || w == JMP_R11).filter(|_| !to_thunk)
            {
                Some(place) => {
                    assert!(upper_half(at + place as u64 + 2), "{form:?} at {offset}");
                    assert!(place >= first);
                    let branch = &bytes[place..place + 3];
                    assert_eq!(branch, if form == Form::Call { CALL_R11 } else { JMP_R11 });
                    // A call is followed by no-ops; a jump by traps.
                    if form == Form::Call {
                        assert_eq!(&bytes[place + 3..], NOPS[3 - place]);
                        assert_eq!(&bytes[..place], NOPS[place]);
                    }
                    if let Form::Branch(_) = form {
                        assert_eq!(bytes[..2], [0x72, (place - 2 + 3) as u8]);
                    }
                }
                None => {
                    assert!(to_thunk, "{form:?} at {offset}: {bytes:02x?}");
                    assert!(!fits_upper(first, SITE - first - 3, 2), "{form:?} {offset}");
                }
            }
        }
        for form in [Form::Jump, Form::Branch(3)] {
            let bytes = rewritten(at, form, THUNKS.ret, Indirect::Plain, Return::Aligned);
            let first = if matches!(form, Form::Branch(_)) { 2 } else { 0 };
            let left = bytes == site(at, form, THUNKS.ret);
            let place = bytes[first..].iter().position(|&b| b == RET).map(|p| p + first);
            match place.filter(|_| !left) {
                Some(place) => {
                    assert!(upper_half(at + place as u64), "{form:?} at {offset}");
                    assert_eq!(&bytes[first..place], NOPS[place - first]);
                    assert!(bytes[place + 1..].iter().all(|&b| b == INT3));
                }
                None => {
                    // Left to the default return thunk, a `ret` in the upper half.
                    assert!(left, "{form:?} at {offset}: {bytes:02x?}");
                    assert!(!fits_upper(first, SITE - first - 1, 0), "{form:?} {offset}");
                }
            }
        }
    }
}

#[test]
fn the_no_ops_are_the_lengths_they_say() {
    for (len, nop) in NOPS.iter().enumerate() {
        assert_eq!(nop.len(), len);
    }
}
