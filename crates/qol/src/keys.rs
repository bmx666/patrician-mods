//! Keyboard: WASD map scrolling, game speed hotkeys.
//!
//! Key state array KS[VK]: WM_KEYDOWN |= 3, WM_KEYUP &= ~2 (bit 1 = held, bit 0 = press event). WINOPEN != 0 while a
//! window is open (text fields live in windows, so typing never scrolls or changes the speed).
use crate::util::*;
use pcore::game::Base;

struct Keys {
    ks: u32,
    winopen: u32,
}

static KEYS: Cell<Keys> = Cell::new();

const VK_CTRL: u32 = 0x11;
const VK_ALT: u32 = 0x12;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_A: u32 = 0x41;
const VK_D: u32 = 0x44;
const VK_S: u32 = 0x53;
const VK_W: u32 = 0x57;

fn keys(base: Base) -> Keys {
    match base {
        Base::P2 => Keys { ks: 0x676838, winopen: 0x66cac4 },
        Base::P3 => Keys { ks: 0x6dcbc0, winopen: 0x6cc3bc },
    }
}

/// 2 if the letter is held and neither Ctrl nor Alt (Ctrl+S = screenshot), else 0.
unsafe fn held(k: &Keys, vk: u32) -> u32 {
    if rd8(k.ks + VK_ALT) & 2 != 0 || rd8(k.ks + VK_CTRL) & 2 != 0 {
        0
    } else {
        (rd8(k.ks + vk) & 2) as u32
    }
}

/// WASD scrolls the town / sea map like the arrow keys. Two readers scroll a map, both skip the arrows while a window
/// is open:
/// - town: 4 blocks "read arrow, latch it, test" (`mov al,[arrow]; ... mov [arrow],dl; je`), 26 bytes each (the P3
///   right-arrow block has an extra `xor ebx,ebx` first: 28) -> the hook latches the arrow as the original and sets
///   ZF from arrow | held letter
/// - sea: all 4 arrows ready at one place (`test edi,edi; mov [right],cl; mov [left],dl; mov [esp+14h],eax; je`):
///   edi = up, ebx = down, [esp+18h] = right, [esp+14h] = left -> ORed with the held letters
pub fn wasd(p: &mut Plan, base: Base) -> Result<(), String> {
    let k = keys(base);
    let (town, sea): ([(u32, u32, u32, bool); 4], u32) = match base {
        Base::P2 => {
            p.check(0x45a150, &cat(&[&hex("a1"), &le(k.winopen), &hex("85c0")]))?; // window gate
            (
                [(0x446e8f, VK_RIGHT, VK_D, false), (0x446f04, VK_LEFT, VK_A, false), (0x446f7e, VK_UP, VK_W, false), (0x446ff8, VK_DOWN, VK_S, false)],
                0x558b06,
            )
        }
        Base::P3 => {
            p.check(0x462310, &cat(&[&hex("8b0d"), &le(k.winopen), &hex("33c085c90f95c0c3")]))?; // window gate
            (
                [(0x44e251, VK_RIGHT, VK_D, true), (0x44e2c9, VK_LEFT, VK_A, false), (0x44e33f, VK_UP, VK_W, false), (0x44e3b5, VK_DOWN, VK_S, false)],
                0x58bc63,
            )
        }
    };
    for (va, arrow, letter, xor_ebx) in town {
        let a = le(k.ks + arrow);
        let block = cat(&[&hex("8ad08ac8d0ea80e27e24fe0ad084c98815"), &a, &hex("74")]);
        let expect = if xor_ebx { cat(&[&hex("a0"), &a, &hex("33db"), &block]) } else { cat(&[&hex("a0"), &a, &block]) };
        let len = if xor_ebx { 28 } else { 26 };
        p.mid(va, len, &expect, &[], &[], wasd_town, arrow | letter << 8 | (xor_ebx as u32) << 16)?;
    }
    let expect = cat(&[&hex("85ff880d"), &le(k.ks + VK_RIGHT), &hex("8815"), &le(k.ks + VK_LEFT), &hex("8944241474")]);
    p.mid(sea, 18, &expect, &[], &[], wasd_sea, 0)?;
    KEYS.set(k);
    Ok(())
}

/// data = arrow VK | letter VK << 8 | xor ebx << 16
unsafe extern "C" fn wasd_town(ctx: *mut Ctx, data: u32) {
    let (c, k) = (&mut *ctx, KEYS.get());
    if data & 0x1_0000 != 0 {
        c.ebx = 0;
    }
    let arrow = k.ks + (data & 0xff);
    let v = rd8(arrow);
    wr8(arrow, ((v >> 1) & 0x7e) | (v & 0xfe));
    c.set_zf((v as u32 | held(k, (data >> 8) & 0xff)) & 0xff == 0);
}

unsafe extern "C" fn wasd_sea(ctx: *mut Ctx, _: u32) {
    let (c, k) = (&mut *ctx, KEYS.get());
    wr8(k.ks + VK_RIGHT, c.ecx as u8);
    wr8(k.ks + VK_LEFT, c.edx as u8);
    *c.stack(0x14) = c.eax;
    if rd32(k.winopen) == 0 {
        c.edi |= held(k, VK_W);
        c.ebx |= held(k, VK_S);
        *c.stack(0x18) |= held(k, VK_D);
        *c.stack(0x14) |= held(k, VK_A);
    }
    c.set_zf(c.edi == 0);
}

const VK_PLUS: u32 = 0xbb; // "=" / "+" of the main keyboard
const VK_MINUS: u32 = 0xbd;
const VK_ADD: u32 = 0x6b; // numpad
const VK_SUBTRACT: u32 = 0x6d;

/// Game speed hotkeys.
/// - Patrician II: + / - (= or numpad +, - or numpad -) = one step faster / slower. Speed index [[66CAF0h]+4]: 0..4 =
///   very slow .. very fast, 5 = extremely slow; order 5 < 0 < 1 < 2 < 3 < 4; setter 4584F0(idx), this = [66C50Ch],
///   with the gates of the Pause key (4231A1). Hook in the game screen key loop (after Pause), key press events are
///   always consumed.
/// - Patrician III has numpad + / - natively (message 0C9h): the main keyboard = / - are mapped onto them.
pub fn speed_keys(p: &mut Plan, base: Base) -> Result<(), String> {
    let k = keys(base);
    match base {
        Base::P2 => {
            p.check(0x4231a1, &hex("a12446680085c0"))?; // mov eax,[684624h]; test eax,eax
            p.check(0x4231aa, &hex("a1303d680085c0"))?; // mov eax,[683D30h]; test eax,eax
            p.check(0x4231b3, &hex("803d04347500ff"))?; // cmp byte [753404h],0FFh
            p.check(0x4231bc, &hex("a1f0ca6600837804"))?; // mov eax,[66CAF0h]; cmp dword [eax+4],..
            p.check(0x4231d2, &hex("8b0d0cc56600e8"))?; // mov ecx,[66C50Ch]; call 4584F0
            p.mid(0x4231e4, 6, &hex("8b96940b0000"), &[], &[], speed_keys_p2, 0)?; // mov edx,[esi+0B94h]
        }
        Base::P3 => {
            // cmp esi,6Bh; je 424C80 / cmp esi,6Dh; je 424C80 (game screen key handler, default path)
            p.check(0x424ba4, &cat(&[&hex("83fe6d"), &jcc_bytes(0x84, 0x424ba4 + 3, 0x424c80)]))?;
            p.mid(0x424b9b, 9, &cat(&[&hex("83fe6b"), &jcc_bytes(0x84, 0x424b9b + 3, 0x424c80)]), &[], &[], speed_keys_p3, 0)?;
        }
    }
    KEYS.set(k);
    Ok(())
}

unsafe extern "C" fn speed_keys_p2(ctx: *mut Ctx, _: u32) {
    let (c, k) = (&mut *ctx, KEYS.get());
    c.edx = rd32(c.esi + 0xb94); // replayed: mov edx,[esi+0B94h]
    let mut step = 0i32;
    for (vk, s) in [(VK_PLUS, 1), (VK_ADD, 1), (VK_MINUS, -1), (VK_SUBTRACT, -1)] {
        let slot = k.ks + vk;
        if rd8(slot) & 1 != 0 {
            wr8(slot, rd8(slot) & 0xfe);
            step = s;
        }
    }
    if step == 0
        || rd8(k.ks + VK_CTRL) & 2 != 0
        || rd8(k.ks + VK_ALT) & 2 != 0
        || rd32(k.winopen) != 0
        || rd32(0x684624) != 0
        || rd32(0x683d30) != 0
        || rd8(0x753404) != 0xff
    {
        return;
    }
    let cur = rd32(rd32(0x66caf0) + 4) as i32;
    let next = if step > 0 {
        let i = if cur == 5 { -1 } else { cur };
        if i >= 4 {
            return;
        }
        i + 1
    } else {
        if cur == 5 {
            return;
        }
        if cur - 1 < 0 { 5 } else { cur - 1 }
    };
    let setter: extern "thiscall" fn(u32, i32) = core::mem::transmute(0x4584f0usize);
    setter(rd32(0x66c50c), next);
}

unsafe extern "C" fn speed_keys_p3(ctx: *mut Ctx, _: u32) {
    let (c, k) = (&mut *ctx, KEYS.get());
    if (c.esi == VK_PLUS || c.esi == VK_MINUS)
        && rd8(k.ks + VK_ALT) & 2 == 0
        && rd8(k.ks + VK_CTRL) & 2 == 0
        && rd32(k.winopen) == 0
    {
        c.esi = if c.esi == VK_PLUS { VK_ADD } else { VK_SUBTRACT };
    }
    // replayed: cmp esi,6Bh; je 424C80 (else the next test at 424BA4)
    c.set_zf(c.esi == VK_ADD);
    c.resume = if c.esi == VK_ADD { 0x424c80 } else { 0x424ba4 };
}
