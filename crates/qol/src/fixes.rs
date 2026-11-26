//! Fixes of original game bugs and small display improvements.
use crate::util::*;
use pcore::game::Base;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Stuck background music / building sounds: the fade-out of the old track (stream slots +4 / +0Ch) steps 1 volume
/// unit every 5000 / target ms with target = CURRENT slider volume; slider low or 0 while a track fades = minutes of
/// the old track. Force target 127 (`jle` -> nops: always `mov ecx,7Fh`): every fade-out ends within 5 s.
pub fn music_fade(p: &mut Plan, base: Base) -> Result<(), String> {
    let sites: [u32; 2] = match base {
        Base::P2 => [0x43e3d8, 0x43e538],
        Base::P3 => [0x444108, 0x444268],
    };
    for va in sites {
        p.patch(va, &hex("83f97f7e05b97f0000"), &hex("83f97f9090b97f0000"))?;
    }
    Ok(())
}

/// New single game screen: the top right title used the "Game settings" string pointer (same as the Details screen).
/// Use the "Single player" pointer of the main menu.
pub fn single_game_title(p: &mut Plan, base: Base) -> Result<(), String> {
    let (va, old, new) = match base {
        Base::P2 => (0x486b75, 0x647df0, 0x647a50),
        Base::P3 => (0x498135, 0x69a950, 0x69a5c4),
    };
    p.patch(va, &cat(&[&hex("8b0d"), &le(old)]), &cat(&[&hex("8b0d"), &le(new)]))
}

/// Building ambience stuck during the tutorial. Ambience (musik\Gebaeude\*.mp3) and the tutorial voice share stream
/// slot +10h; while the tutorial object exists (`[esi+1DEh]` = 1) leaving a building does not close the slot, so the
/// ambience keeps playing and no other building's ambience starts. Remember who owns the slot (OWN = building
/// ambience): on leaving a building close it whenever it is ambience, and let ambience loop as outside the tutorial.
///
/// - A (slot opened past this point, `cmp di,0FFFFh; jne`): OWN = id != 0FFFFh (tutorial voice id)
/// - B (`cmp [esi+1DEh],bl; jne` skip "loop count 0"): loop if no tutorial (original) or OWN
/// - C (`mov al,[esi+1DEh]; test al,al; jne` skip close): close if no tutorial (original) or OWN
pub fn tutorial_ambience(p: &mut Plan, base: Base) -> Result<(), String> {
    let a = match base {
        Base::P2 => Amb { a: 0x43c598, a_jne: 0x43c5b5, b: 0x43c71e, b_loop: 0x43c726, c: 0x43db23, c_skip: 0x43db4f },
        Base::P3 => Amb { a: 0x4422b8, a_jne: 0x4422d5, b: 0x44243e, b_loop: 0x442446, c: 0x443857, c_skip: 0x443883 },
    };
    p.mid(a.a, 7, &hex("6681ffffff7516"), &[], &[], amb_a, 0)?;
    p.mid(a.b, 8, &hex("389ede010000750b"), &[], &[], amb_b, 0)?;
    p.mid(a.c, 10, &hex("8a86de01000084c07522"), &[], &[], amb_c, 0)?;
    AMB.set(a);
    Ok(())
}

struct Amb {
    a: u32,
    a_jne: u32,
    b: u32,
    b_loop: u32,
    c: u32,
    c_skip: u32,
}

static AMB: Cell<Amb> = Cell::new();
static OWN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn amb_a(ctx: *mut Ctx, _: u32) {
    let (c, a) = (&mut *ctx, AMB.get());
    let own = c.edi as u16 != 0xffff;
    OWN.store(own, Ordering::Relaxed);
    c.set_zf(!own);
    c.resume = if own { a.a_jne } else { a.a + 7 };
}

unsafe extern "C" fn amb_b(ctx: *mut Ctx, _: u32) {
    let (c, a) = (&mut *ctx, AMB.get());
    let tutorial = rd8(c.esi + 0x1de);
    // b_loop = b + 8 (original fall-through: set loop count 0), b + 13h = original jne target (skip it)
    c.resume = if tutorial == c.ebx as u8 || OWN.load(Ordering::Relaxed) { a.b_loop } else { a.b + 0x13 };
}

unsafe extern "C" fn amb_c(ctx: *mut Ctx, _: u32) {
    let (c, a) = (&mut *ctx, AMB.get());
    let tutorial = rd8(c.esi + 0x1de);
    c.eax = (c.eax & !0xff) | tutorial as u32;
    c.resume = if tutorial != 0 && !OWN.load(Ordering::Relaxed) { a.c_skip } else { a.c + 10 };
}

/// Shipyard row "knots": the byte table (P2 6/7/4/5, P3 6/6/5/5 for snaikka / crayer / cog / holk) differs from the
/// real base speed (sailing table = knots * 115.5: 5.5 / 6 / 4.5 / 5). Table := knots * 2 (11 / 12 / 9 / 10); its only
/// reader (sprintf "%i") goes through [`knots_sprintf`]: v / 2, odd -> "%i.5". The default 0Ah is doubled too.
pub fn shipyard_knots(p: &mut Plan, base: Base) -> Result<(), String> {
    let (table, site, sprintf, default) = match base {
        Base::P2 => {
            p.check(0x623490, &hex("06070405"))?;
            (0x623490, 0x5aa409, 0x5f85f0, 0x5a8b8b)
        }
        Base::P3 => {
            p.check(0x673668, &hex("06060505 7b02b50208024202"))?;
            p.check(0x5f5843, &cat(&[&hex("68"), &le(0x6c7470), &hex("50")]))?;
            (0x673668, 0x5f5849, 0x64df80, 0x5f3ed9)
        }
    };
    let def_bytes = match base {
        Base::P2 => hex("c686dc0c00000a"),
        Base::P3 => hex("c686f00c00000a"),
    };
    p.check(site, &call_bytes(site, sprintf))?;
    p.patch(default, &def_bytes, &cat(&[&def_bytes[..6], &[0x14]]))?;
    p.write(table, vec![11, 12, 9, 10]);
    p.write(site, call_bytes(site, knots_sprintf as *const () as usize as u32));
    SPRINTF.store(sprintf, Ordering::Relaxed);
    Ok(())
}

static SPRINTF: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" fn knots_sprintf(buf: u32, fmt: *const u8, v: u32) -> i32 {
    let f: Sprintf = core::mem::transmute(SPRINTF.load(Ordering::Relaxed) as usize);
    if v & 1 != 0 {
        f(buf, b"%i.5\0".as_ptr(), v >> 1)
    } else {
        f(buf, fmt, v >> 1)
    }
}

/// Campaign start keeps the speed index of the settings but never sends message 0A0h, so the game runs with the tick
/// stored in the campaign savegame. `mov eax,[66CAF0h]; mov edx,[ecx+15Ch]; mov [eax+4],edx` (ecx = [66C50Ch]) ->
/// `push dword [ecx+15Ch]; call 4584F0` (speed setter, thiscall: writes the index and sends 0A0h). Patrician II only
/// (Patrician III changes the speed with message 0C9h).
pub fn campaign_speed(p: &mut Plan, _: Base) -> Result<(), String> {
    p.check(0x497ea8, &hex("8b0d0cc56600"))?;
    let new = cat(&[&hex("ffb15c010000"), &rel5(0xe8, 0x497eae + 6, 0x4584f0), &hex("909090")]);
    p.patch(0x497eae, &hex("a1f0ca66008b915c010000895004"), &new)
}

/// Ship panel: current speed in tenths of a knot. Field `[ship+30h]` is the speed in 1/16 knot; the game printed
/// v >> 4 with "%u". The hook prints t = (v*10 + 15) / 16 (rounded up: the game truncates, a snaikka at full speed
/// would show 5.4 instead of 5.5) as "%u" when t % 10 = 0 (harbour "0"), else "%u.%u". The original pushed 3 sprintf
/// arguments (cleaned up later with other values): the hook site keeps that stack with 3 dummy pushes.
pub fn ship_speed(p: &mut Plan, base: Base) -> Result<(), String> {
    let (site, sprintf, fu) = match base {
        Base::P2 => (0x481224, 0x5f85f0, 0x65a630),
        Base::P3 => (0x491db2, 0x64df80, 0x6ba468),
    };
    // mov ecx,[eax+30h]; sar ecx,4; push ecx; push "%u"; push edx; call sprintf
    let expect = cat(&[&hex("8b4830c1f9045168"), &le(fu), &hex("52"), &call_bytes(site + 13, sprintf)]);
    p.check(fu, &hex("257500"))?;
    p.mid(site, 18, &expect, &[], &hex("6a006a006a00"), ship_speed_hook, 0)?;
    SHIP.set((sprintf, fu));
    Ok(())
}

static SHIP: Cell<(u32, u32)> = Cell::new();

/// eax = ship, edx = buffer.
unsafe extern "C" fn ship_speed_hook(ctx: *mut Ctx, _: u32) {
    let c = &mut *ctx;
    let (sprintf, fu) = *SHIP.get();
    let f: Sprintf = core::mem::transmute(sprintf as usize);
    let t = rd32(c.eax + 0x30).wrapping_mul(10).wrapping_add(15) >> 4;
    let (int, frac) = (t / 10, t % 10);
    let fmt = if frac == 0 { fu as *const u8 } else { b"%u.%u\0".as_ptr() };
    c.eax = f(c.edx, fmt, int, frac) as u32;
}
