//! Game screen: game speed plate centred below the top bar, 6 icon buttons (no text), one per speed: II, II>, I>, >,
//! >>, >>> (slowest .. fastest). The current speed is drawn as a pressed (dark) button; a click sets that speed, with
//! the same effect and the same multiplayer gates as the in-game options. Locked (multiplayer client) = grey icons.
//!
//! - draw: game screen draw, hooked before its render target restore; top bar element by screen width (800 / 1024 /
//!   else, as the game's layout function); plate position from the element's position and size (vtable +44h / +48h)
//! - click: the WM_LBUTTONDOWN handler is hooked at its entry; a click on the plate while the game screen is alive
//!   (its update ran < 300 ms ago, screen width unchanged since the plate was drawn) is swallowed (`ret 0Ch`) and
//!   applied by the next game screen update
//! - update (game screen vtable +12Ch, hooked at entry): applies a click; invalidates the plate area plus the
//!   scroll margin every frame (the map views scroll by copying the picture, plate included, and repaint only the
//!   exposed strip; the repaint draws the scene there, then the draw hook puts the plate back)
//!
//! Patrician II: speed index [[66CAF0h]+4] (order slowest -> fastest 5,0,1,2,3,4); a click writes the index, the host
//! also sends message 0A0h with the tick. Patrician III: the native hotkeys change only the tick [6DFBC4h], so the
//! pressed button is the one whose tick matches; a click writes the index and sends message 0C8h with the tick.
use crate::util::*;
use pcore::ffi::GetTickCount;
use pcore::game::Base;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, Ordering::Relaxed};

const NB: i32 = 6;
const BW: i32 = 28;
const H: i32 = 22;
const TOT: i32 = NB * BW + 1; // buttons at x = 1 + p*BW, width BW-1 (1 px edge between)
const TRX: i32 = 96; // repaint margin (max scroll step per frame)
const TRY: i32 = 64;
const COL_EDGE: u32 = 0xff3a2a1a;
const COL_BG: u32 = 0xffe6d3a3;
const COL_BTN: u32 = 0xffc4a66e;
const COL_INK: u32 = 0xff2a1c10;
const COL_OFF: u32 = 0xff9c8c70;
/// Button p -> speed index.
const IDX: [i32; 6] = [5, 0, 1, 2, 3, 4];

struct Plate {
    p3: bool,
    set_color: u32,
    fill: u32,
    invalidate: u32,
    inv_this: u32,
    net_send: u32,
    net_obj: u32,
    mx: u32,
    my: u32,
    ks: u32,
    screen: u32,
    scrw: u32,
    game: u32,
    net_id: u32,
    net_blk: u32,
    net_st: u32,
    tick: u32,
    factab: u32,
    /// Top bar element offsets in the game screen for widths 800, 1024, other.
    el: [u32; 3],
    /// Patrician III: tick of each button.
    ticks: [i32; 6],
    /// Glyph rects {dx, dy, w, h} of each button, relative to the plate.
    glyphs: Vec<Vec<[i32; 4]>>,
    ret_0c: u32,
}

static PLATE: Cell<Plate> = Cell::new();
static SL: AtomicI32 = AtomicI32::new(0);
static ST: AtomicI32 = AtomicI32::new(0);
static STAMP: AtomicU32 = AtomicU32::new(0);
static SW: AtomicU32 = AtomicU32::new(0);
static ON: AtomicBool = AtomicBool::new(false);
static CLICK: AtomicU8 = AtomicU8::new(0);
static RECT: [AtomicI32; 4] = [AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)];

/// Glyphs: b = bar 2x10, t = triangle of 5 columns (heights 10, 8, 6, 4, 2); gap 2 after a bar, 1 after a triangle.
fn glyphs() -> Vec<Vec<[i32; 4]>> {
    const GH: i32 = 10;
    const GY: i32 = (H - GH) >> 1;
    ["bb", "bbt", "bt", "t", "tt", "ttt"]
        .iter()
        .enumerate()
        .map(|(p, g)| {
            let n = g.len();
            let w: i32 = g
                .bytes()
                .enumerate()
                .map(|(i, e)| (if e == b'b' { 2 } else { 5 }) + if i + 1 < n { if e == b'b' { 2 } else { 1 } } else { 0 })
                .sum();
            let mut x = 1 + p as i32 * BW + ((BW - 1 - w) >> 1);
            let mut r = Vec::new();
            for e in g.bytes() {
                if e == b'b' {
                    r.push([x, GY, 2, GH]);
                    x += 4;
                } else {
                    for k in 0..5 {
                        r.push([x + k, GY + k, 1, GH - 2 * k]);
                    }
                    x += 6;
                }
            }
            r
        })
        .collect()
}

pub fn speed_plate(p: &mut Plan, base: Base) -> Result<(), String> {
    let ret_0c = Plan::ret_stub(3)?;
    let plate = match base {
        Base::P2 => {
            p.check32(0x61c780 + 0x9c, 0x426610)?; // game screen vtable: draw
            p.check32(0x61c780 + 0x12c, 0x422330)?; // update
            p.mid(0x426b27, 6, &cat(&[&hex("55"), &call_bytes(0x426b28, 0x4a7bd0)]), // push ebp; call 4A7BD0
                &[Ins::Bytes(&[0x55]), Ins::Call(0x4a7bd0)], &[], plate_draw, 0)?;
            p.mid(0x422330, 6, &hex("64a100000000"), &[Ins::Bytes(&[0x64, 0xa1, 0, 0, 0, 0])], &[], plate_update, 0)?;
            p.mid(0x4acf00, 7, &cat(&[&hex("800d"), &le(0x676838 + 1), &hex("03")]), &[], &[], plate_mouse_down, 0)?;
            Plate {
                p3: false,
                set_color: 0x4a7a90,
                fill: 0x4a7650,
                invalidate: 0x4a5870,
                inv_this: 0x674268,
                net_send: 0x514610,
                net_obj: 0x683d10,
                mx: 0x6747c8,
                my: 0x6747c0,
                ks: 0x676838,
                screen: 0x66c26c,
                scrw: 0x676958,
                game: 0x66caf0,
                net_id: 0x683d30,
                net_blk: 0x684624,
                net_st: 0,
                tick: 0,
                factab: 0x61dc5c,
                el: [0xc78, 0xd50, 0xe28],
                ticks: [0; 6],
                glyphs: glyphs(),
                ret_0c,
            }
        }
        Base::P3 => {
            const SCR_VT: u32 = 0x66c8a0;
            const SCREEN: u32 = 0x6cbb40;
            const GAME: u32 = 0x6cc3e8;
            const FACTAB: u32 = 0x66de7c;
            const NETOBJ: u32 = 0x6df2f0;
            const NET_SEND: u32 = 0x54aa70;
            const NETBLK: u32 = 0x6dfc18;
            const NETID: u32 = 0x6df31c;
            const NETST: u32 = 0x6e59cc;
            const TICK: u32 = 0x6dfbc4;
            const KS: u32 = 0x6dcbc0;
            p.check32(SCR_VT + 0x9c, 0x4285c0)?;
            p.check32(SCR_VT + 0x12c, 0x423840)?;
            p.check(0x423312, &cat(&[&hex("c706"), &le(SCR_VT)]))?; // game screen ctor stores the vtable
            p.check(0x463ace, &cat(&[&hex("a3"), &le(SCREEN)]))?; // ... and the object goes to [6CBB40h]
            p.check(0x45df36, &hex("8b8e7c010000"))?; // options accept: mov ecx,[esi+17Ch] (speed index)
            p.check(0x45df4d, &cat(&[&hex("a1"), &le(GAME), &hex("894804")]))?; // mov eax,[6CC3E8h]; mov [eax+4],ecx
            p.check(0x45df6d, &hex("c7442414c8000000"))?; // message 0C8h
            p.check(0x45df7d, &cat(&[&hex("8b048d"), &le(FACTAB)]))?; // factor table
            p.check(0x45df84, &cat(&[&hex("b9"), &le(NETOBJ)]))?;
            p.check(0x45dfae, &call_bytes(0x45dfae, NET_SEND))?;
            p.check(0x424c89, &cat(&[&hex("a1"), &le(NETBLK), &hex("85c0754da1"), &le(NETID), &hex("85c07544803d"), &le(NETST), &hex("ff")]))?;
            p.check(0x546e12, &hex("3b86d4080000"))?; // speed message handler: tick = this+8D4h
            p.check(0x42d23e, &cat(&[&hex("a1"), &le(TICK)]))?; // tick read by address
            p.check(0x429a47, &hex("8d86e0080000"))?; // top bar elements 800 / 1024 / else
            p.check(0x429a53, &hex("8d86b8090000"))?;
            p.check(0x429a61, &hex("8d86900a0000"))?;
            // button p -> tick = factor * 1000 / 256 (as the options accept)
            let mut ticks = [0i32; 6];
            for (t, i) in ticks.iter_mut().zip(IDX) {
                *t = (unsafe { rd32(FACTAB + 4 * i as u32) } as i32 * 1000) >> 8;
            }
            if ticks != [3515, 468, 351, 234, 117, 78] {
                return Err(format!("factor table differs: {ticks:?}"));
            }
            // mov ecx,[esp+18h]; push ecx; call 4BB9B0
            p.mid(0x428bfa, 10, &cat(&[&hex("8b4c241851"), &call_bytes(0x428bff, 0x4bb9b0)]),
                &[Ins::Bytes(&[0x8b, 0x4c, 0x24, 0x18, 0x51]), Ins::Call(0x4bb9b0)], &[], plate_draw, 0)?;
            p.mid(0x423840, 6, &hex("64a100000000"), &[Ins::Bytes(&[0x64, 0xa1, 0, 0, 0, 0])], &[], plate_update, 0)?;
            p.mid(0x4c0c20, 7, &cat(&[&hex("800d"), &le(KS + 1), &hex("03")]), &[], &[], plate_mouse_down, 0)?;
            Plate {
                p3: true,
                set_color: 0x4bb870,
                fill: 0x4bb430,
                invalidate: 0x4b9650,
                inv_this: 0x6da5f0,
                net_send: NET_SEND,
                net_obj: NETOBJ,
                mx: 0x6dab50,
                my: 0x6dab48,
                ks: KS,
                screen: SCREEN,
                scrw: 0x6dcce0,
                game: GAME,
                net_id: NETID,
                net_blk: NETBLK,
                net_st: NETST,
                tick: TICK,
                factab: FACTAB,
                el: [0x8e0, 0x9b8, 0xa90],
                ticks,
                glyphs: glyphs(),
                ret_0c,
            }
        }
    };
    PLATE.set(plate);
    Ok(())
}

impl Plate {
    unsafe fn set_color(&self, c: u32) -> u32 {
        let f: extern "C" fn(u32) -> u32 = core::mem::transmute(self.set_color as usize);
        f(c)
    }

    unsafe fn fill(&self, x: i32, y: i32, w: i32, h: i32) {
        let f: extern "C" fn(i32, i32, i32, i32) -> i32 = core::mem::transmute(self.fill as usize);
        f(x, y, w, h);
    }

    /// Speed may be changed (single player or host), as the game's own gates.
    unsafe fn can_change(&self) -> bool {
        if self.p3 {
            rd32(self.net_blk) == 0 && rd32(self.net_id) == 0 && rd8(self.net_st) == 0xff
        } else {
            let id = rd32(self.net_id) as i32;
            (id == 0 || id == -1) && rd32(self.net_blk) == 0
        }
    }

    unsafe fn pressed(&self, p: usize) -> bool {
        if self.p3 {
            rd32(self.tick) as i32 == self.ticks[p]
        } else {
            rd32(rd32(self.game) + 4) as i32 == IDX[p]
        }
    }

    unsafe fn send(&self, msg: [i32; 5]) {
        let f: extern "thiscall" fn(u32, *const i32) = core::mem::transmute(self.net_send as usize);
        f(self.net_obj, msg.as_ptr());
    }
}

unsafe extern "C" fn plate_draw(_: *mut Ctx, _: u32) {
    let pl = PLATE.get();
    let screen = rd32(pl.screen);
    if screen == 0 {
        return;
    }
    let w = rd32(pl.scrw);
    let el = screen + match w {
        800 => pl.el[0],
        1024 => pl.el[1],
        _ => pl.el[2],
    };
    let vt = rd32(el);
    let width: extern "thiscall" fn(u32) -> i32 = core::mem::transmute(rd32(vt + 0x44) as usize);
    let height: extern "thiscall" fn(u32) -> i32 = core::mem::transmute(rd32(vt + 0x48) as usize);
    let l = (width(el) >> 1) + rd32(el + 0x20) as i32 - (TOT >> 1);
    let t = height(el) + rd32(el + 0x24) as i32 + 4;
    SL.store(l, Relaxed);
    ST.store(t, Relaxed);
    ON.store(true, Relaxed);
    SW.store(w, Relaxed);
    let old = pl.set_color(COL_EDGE);
    pl.fill(l, t, TOT, H);
    let can = pl.can_change();
    for (p, glyph) in pl.glyphs.iter().enumerate() {
        let x = l + 1 + p as i32 * BW;
        if pl.pressed(p) {
            pl.set_color(COL_INK);
            pl.fill(x, t + 1, BW - 1, H - 2);
            pl.set_color(COL_BG);
        } else {
            pl.set_color(COL_BTN);
            pl.fill(x, t + 1, BW - 1, H - 2);
            pl.set_color(if can { COL_INK } else { COL_OFF });
        }
        for r in glyph {
            pl.fill(l + r[0], t + r[1], r[2], r[3]);
        }
    }
    pl.set_color(old);
}

/// WM_LBUTTONDOWN handler entry (stdcall, 3 arguments): replays `or byte [KS+1],3`; a click on the plate returns
/// at once.
unsafe extern "C" fn plate_mouse_down(ctx: *mut Ctx, _: u32) {
    let (c, pl) = (&mut *ctx, PLATE.get());
    wr8(pl.ks + 1, rd8(pl.ks + 1) | 3);
    if !ON.load(Relaxed)
        || GetTickCount().wrapping_sub(STAMP.load(Relaxed)) >= 300
        || rd32(pl.scrw) != SW.load(Relaxed)
    {
        return;
    }
    let dx = rd32(pl.mx).wrapping_sub(SL.load(Relaxed) as u32);
    let dy = rd32(pl.my).wrapping_sub(ST.load(Relaxed) as u32);
    if dx >= TOT as u32 || dy >= H as u32 {
        return;
    }
    let button = (dx as i32 - 1).max(0) / BW;
    CLICK.store(button as u8 + 1, Relaxed);
    c.resume = pl.ret_0c;
}

/// Game screen update entry (ecx = game screen).
unsafe extern "C" fn plate_update(_: *mut Ctx, _: u32) {
    let pl = PLATE.get();
    STAMP.store(GetTickCount(), Relaxed);
    let click = CLICK.swap(0, Relaxed);
    if click != 0 && pl.can_change() {
        let button = click as usize - 1;
        if pl.p3 {
            if button < NB as usize && pl.ticks[button] != rd32(pl.tick) as i32 {
                wr32(rd32(pl.game) + 4, IDX[button] as u32);
                pl.send([0xc8, pl.ticks[button], -1, -1, -1]);
            }
        } else if button < NB as usize {
            let g = rd32(pl.game);
            let idx = IDX[button];
            if rd32(g + 4) as i32 != idx {
                wr32(g + 4, idx as u32);
                if rd32(pl.net_id) == 0 {
                    // host: network message as the options accept
                    let tick = rd32(pl.factab + 4 * idx as u32) as i32 * 1000 / 256;
                    pl.send([0xa0, tick, -1, -1, -1]);
                }
            }
        }
    }
    if ON.load(Relaxed) && rd32(pl.scrw) == SW.load(Relaxed) {
        let (l, t) = (SL.load(Relaxed), ST.load(Relaxed));
        for (r, v) in RECT.iter().zip([l - TRX, t - 1, l + TOT + TRX, t + H + TRY]) {
            r.store(v, Relaxed);
        }
        let f: extern "thiscall" fn(u32, *const AtomicI32) = core::mem::transmute(pl.invalidate as usize);
        f(pl.inv_this, RECT.as_ptr());
    }
}
