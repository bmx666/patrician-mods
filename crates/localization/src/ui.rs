//! Window layouts for translated labels longer than the English ones.
use pcore::game::Base;
use pcore::hook::Plan;
use std::path::Path;

/// Patrician II auto-trade window (Autotradeparchment, layout 406bea), bottom row:
/// - Undo (+3D0h, created 404339 with buttons.ini `Button[61DC36h]` = Button2, 48x18): translations (Отмена,
///   Atšaukti ...) overflow it -> Button6 (72x18), defined by the language's `scripts\buttons.ini`;
///   x = NextCityPos + ">" width + 20 (unchanged, grows right);
/// - "trade with office" toggle (+590h) + label: x = Undo x + 1Eh -> + 46h (40 px right);
/// - "trade with town" toggle (+4B8h) + label: x = PrevCityPos x -> x - 20 (y - 1Eh as before; esi dead after).
///
/// Needs `[Button6]` in `<lang dir>\scripts\buttons.ini` (the game would draw no Undo button without it).
pub fn trade_row(p: &mut Plan, base: Base, lang_dir: Option<&Path>) -> Result<(), String> {
    if base != Base::P2 {
        return Err("Patrician II only".into());
    }
    if !has_button6(lang_dir) {
        return Err("no [Button6] in scripts\\buttons.ini of the language".into());
    }
    // mov al,[61DC36h] -> mov al,6
    p.patch(0x40433b, &[0xa0, 0x36, 0xdc, 0x61, 0x00], &[0xb0, 0x06, 0x90, 0x90, 0x90])?;
    // add eax,1Eh -> add eax,46h
    p.patch(0x406cb5, &[0x83, 0xc0, 0x1e], &[0x83, 0xc0, 0x46])?;
    // mov esi,[esp+14h]; add ebp,4B8h; sub esi,1Eh; push ebp; mov ecx,ebx; mov [esp+18h],esi ->
    // sub dword [esp+14h],1Eh; sub dword [esp+10h],14h; add ebp,4B8h; push ebp; mov ecx,ebx; nop
    p.patch(
        0x406cfb,
        &[
            0x8b, 0x74, 0x24, 0x14, 0x81, 0xc5, 0xb8, 0x04, 0x00, 0x00, 0x83, 0xee, 0x1e, 0x55, 0x8b, 0xcb, 0x89, 0x74,
            0x24, 0x18,
        ],
        &[
            0x83, 0x6c, 0x24, 0x14, 0x1e, 0x83, 0x6c, 0x24, 0x10, 0x14, 0x81, 0xc5, 0xb8, 0x04, 0x00, 0x00, 0x55, 0x8b,
            0xcb, 0x90,
        ],
    )
}

fn has_button6(lang_dir: Option<&Path>) -> bool {
    let Some(t) = lang_dir.and_then(|d| std::fs::read(d.join("scripts").join("buttons.ini")).ok()) else {
        return false;
    };
    t.windows(9).any(|w| w.eq_ignore_ascii_case(b"[button6]"))
}
