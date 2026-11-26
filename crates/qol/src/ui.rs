//! Window layout improvements.
use crate::util::*;
use pcore::game::Base;

const BACK_A: u32 = 0xdc; // 86 %

/// Patrician II building windows (church, office, smith, ...): the interior picture is covered by a beige D2C18Ah
/// backdrop: a top fade of 82h rows with alpha = row, then one FillSolidRect with alpha 82h (51 % opaque) -> black
/// text hard to read. Solid alpha := BACK_A, fade scaled to it (alpha = row * BACK_A / 82h).
pub fn building_backdrop(p: &mut Plan, _: Base) -> Result<(), String> {
    p.check(0x45d828, &hex("0d8ac1d200"))?; // or eax,0D2C18Ah (follows the hook)
    p.patch(0x45d883, &hex("688ac1d282"), &cat(&[&hex("688ac1d2"), &[BACK_A as u8]]))?; // push 82D2C18Ah
    p.mid(0x45d820, 8, &hex("25ff000000c1e018"), &[], &[], backdrop_fade, 0) // and eax,0FFh; shl eax,18h
}

unsafe extern "C" fn backdrop_fade(ctx: *mut Ctx, _: u32) {
    let c = &mut *ctx;
    c.eax = ((c.eax & 0xff) * BACK_A / 0x82) << 24;
}

/// Patrician III trade route file dialog (list dialog mode 4): Load / Save / Delete use the large button style 3
/// (96x18) instead of the medium style 2 (48x18), and the window gets the minimum width MINW (title or file names may
/// still widen it).
pub fn trade_route_dialog(p: &mut Plan, _: Base) -> Result<(), String> {
    const MINW: u16 = 340;
    p.check(0x66de55, &hex("0302"))?; // style bytes: large 3, medium 2
    for va in [0x4708e9, 0x470921, 0x470959] {
        p.patch(va, &cat(&[&hex("8a0d"), &le(0x66de56)]), &cat(&[&hex("8a0d"), &le(0x66de55)]))?;
    }
    p.check(0x46fa2d, &hex("eb07"))?; // jmp short 46fa36
    p.mid(0x46fa2f, 7, &hex("66899ee4000000"), &[], &[], route_width, MINW as u32) // mov [esi+0E4h],bx
}

unsafe extern "C" fn route_width(ctx: *mut Ctx, minw: u32) {
    wr16((*ctx).esi + 0xe4, minw as u16);
}
