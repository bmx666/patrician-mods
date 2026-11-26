//! Widescreen, Patrician II and III (port of the patrician2 / patrician3 `tools/exepatch.js` widescreen patch). The
//! third resolution of the options ("1280 x 1024", index 2) becomes one of the presets below; `mods\settings.ini`
//! `[display] width / height` selects it at start (0 / missing = 1280x1024, the original size; a valid size that is
//! not in the list is added to it) and is written back when the options are saved. The options list steps through
//! all presets at index 2. P3 is the P2 code with shifted addresses and object offsets ([`Addr`]).
//!
//! The game screen layout is mostly relative to the current size (screen width / height); 3 width tests `== 1280`
//! become `> 1024`. The 1280 pieces are switched per preset: top bar anim, bottom right carving anim, overview map
//! screen; the fast-travel overview map is centred at its own size. The frame pictures and ini entries of every preset
//! are made at run time from the game's own files ([`data`]).
//!
//! The game screen is built for the current preset G when a game is started / loaded, so G changes only then: the
//! options list changes PIDX (+ label); COPYG (G = preset PIDX) runs at the mode switch sites and at the screen build;
//! the layout function re-inits the top bar / carving when the preset changed since the build.
use crate::util::*;
use pcore::game::Base;
use pcore::{ini::Ini, log, paths};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

mod data;

/// Built-in presets (width, height).
const LIST: [(u32, u32); 33] = [
    // 4:3
    (1152, 864), (1280, 960), (1400, 1050), (1600, 1200), (1920, 1440), (2048, 1536), (2880, 2160),
    // 5:4
    (1280, 1024),
    // 16:10
    (1280, 800), (1440, 900), (1680, 1050), (1920, 1200), (2560, 1600), (2880, 1800), (3840, 2400),
    // 16:9
    (1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3200, 1800), (3840, 2160),
    // 21:9 / 32:9 / DCI 4K
    (2560, 1080), (3440, 1440), (3840, 1600), (3840, 1080), (4096, 2160),
    // Mac (logical "looks like" and native Retina sizes)
    (1470, 956), (1512, 982), (1728, 1117), (2560, 1664), (3024, 1964), (3456, 2234),
];

/// Original size of resolution index 2; used when `[display]` is missing, 0 or invalid.
const DEFAULT: (u32, u32) = (1280, 1024);

/// Bytes per preset table entry: dd W, H, top anim, carving anim, accel screen; dq overview map scale x, y,
/// offset x, y; dd overview map right edge, top edge.
const ESZ: u32 = 60;

/// Anim id store of the screen build (`pre; mov [ebp+disp],eax; post; call`): the anim id of G is stored instead and
/// remembered as the loaded one.
struct Store {
    va: u32,
    pre: &'static str,
    disp: u8,
    post: &'static str,
    /// Callee of a trailing `call rel32` (0 = none).
    call: u32,
}

/// Code sites of one game.
struct Addr {
    /// Current screen width (dd).
    scrw: u32,
    /// IAT entry of wsprintfA.
    wsprintfa: u32,
    /// Options label "1280 x 1024" (11 characters + NUL, "Invert stereo" follows).
    label: u32,
    /// CString::CString(const char *).
    cstring: u32,
    /// First free ANIM id of screenGame.ini.
    anim0: u32,
    /// Options resolution list (helper object ecx, [ecx+4] = index): next / previous (`8b410456be01000000`), button
    /// enables (bytes [esp+10h..13h]) and its site `xor esi,esi; lea edi,[ecx+0Ch]`.
    next: u32,
    prev: u32,
    buttons: u32,
    buttons_site: u32,
    /// Options save (after the resolution write), its original 10 bytes.
    save: u32,
    save_orig: &'static str,
    /// Options opened (backs up the resolution) / reset to the backup: 6-byte `mov r32,[r32+disp32]` sites.
    opened: u32,
    opened_orig: &'static str,
    reset: u32,
    reset_orig: &'static str,
    /// Mode switch sites `mov dword [esp+X],500h; mov dword [esp+Y],400h`: (va, X, Y).
    modes: [(u32, u8, u8); 5],
    /// Screen build: top bar / carving anim id.
    top: Store,
    carv: Store,
    /// Game screen layout function (prologue `515355568bf1`), top bar / carving element offsets.
    layout: u32,
    el_top: u32,
    el_carv: u32,
    /// Overview map screen id `push 2; push ecx; mov ecx,esp`.
    accel: u32,
    /// Fast-travel map: x scale offset (y = +8), its `fstp qword [ecx+y]` site, `fmul qword [esi+x/y]` sites.
    scale_x: u32,
    scale_site: u32,
    fmul: [(u32, bool); 6],
    /// Gold/date box runs: (va, bytes, offset of `push y`); each starts with `mov r32,[esi+2Ch]`.
    gold: [(u32, &'static str, usize); 7],
    /// Gold box redraw rect `mov ebp,28h; cmp edx,eax; mov edi,4Bh` and its `mov eax,[esi+2Ch]; lea edx,[esp+44h]`.
    rect: u32,
    rect_x: [u32; 2],
    /// Width tests: (va, original, patched).
    width_tests: [(u32, &'static str, &'static str); 3],
}

const P2: Addr = Addr {
    scrw: 0x676958,
    wsprintfa: 0x61a4f0,
    label: 0x6482f8,
    cstring: 0x5fa7ad,
    anim0: 79,
    next: 0x402d20,
    prev: 0x402d70,
    buttons: 0x402c00,
    buttons_site: 0x402cfa,
    save: 0x456720,
    save_orig: "8b8e480100008d542410",
    opened: 0x456ab9,
    opened_orig: "8b8d48010000",
    reset: 0x45826c,
    reset_orig: "8b968c010000",
    modes: [(0x4226de, 0x3c, 0x40), (0x42b026, 0x18, 0x1c), (0x42f3f7, 0x3c, 0x40), (0x457e79, 0x48, 0x4c), (0x45bae9, 0x24, 0x28)],
    top: Store { va: 0x42387f, pre: "8b4de8", disp: 0xb8, post: "8b4508", call: 0 },
    carv: Store { va: 0x423918, pre: "52", disp: 0xbc, post: "", call: 0x5fa4b4 },
    layout: 0x427f20,
    el_top: 0xe28,
    el_carv: 0x1188,
    accel: 0x428057,
    scale_x: 0x248,
    scale_site: 0x401339,
    fmul: [(0x4015c4, false), (0x4015ec, true), (0x401e75, true), (0x401ea9, false), (0x401ee9, true), (0x401f1e, false)],
    gold: [
        (0x40165b, "8b4e2c508b4424402bc85083e9246a2c", 14),
        (0x4016be, "8b4e2c83c404508b4424302bc85083e9286a28", 17),
        (0x4016f0, "8b4e2c8b7c24686a01508b4424346a31", 14),
        (0x40173e, "8b4e2c6a01508b4424346a32", 10),
        (0x401801, "8b562cd1f82bd0555383ea326a34", 12),
        (0x40182d, "8b4e2c6a02508b4424346a31", 10),
        (0x401877, "8b4e2c8bb6440200006a02508b4424346a32", 16),
    ],
    rect: 0x40207e,
    rect_x: [0x40208c, 0x40211a],
    width_tests: [
        (0x40146b, "3d00050000b9984467007411", "3d00040000b9984467007711"),
        (0x427fbb, "3d000500000f85ff010000", "3d00050000909090909090"),
        (0x4284ac, "3d000500007545", "3d000500009090"),
    ],
};

const P3: Addr = Addr {
    scrw: 0x6dcce0,
    wsprintfa: 0x66a3d0,
    label: 0x69ad98,
    cstring: 0x64f2c1,
    anim0: 113,
    next: 0x402cc0,
    prev: 0x402d10,
    buttons: 0x402ba0,
    buttons_site: 0x402c9a,
    save: 0x45e2b6,
    save_orig: "8b8e880100008d542410",
    opened: 0x45e6ce,
    opened_orig: "8b8d68010000",
    reset: 0x4602bc,
    reset_orig: "8b96b8010000",
    modes: [(0x423bf0, 0x4c, 0x50), (0x42d168, 0x18, 0x1c), (0x432d36, 0x3c, 0x40), (0x45fe68, 0x48, 0x4c), (0x463fed, 0x24, 0x28)],
    top: Store { va: 0x425071, pre: "8b5508", disp: 0xbc, post: "", call: 0 },
    carv: Store { va: 0x4250fd, pre: "", disp: 0xdc, post: "8b450c", call: 0 },
    layout: 0x429a40,
    el_top: 0xa90,
    el_carv: 0xdf0,
    accel: 0x429b77,
    scale_x: 0x268,
    scale_site: 0x401339,
    fmul: [(0x4015de, false), (0x4015f8, true), (0x401e9a, true), (0x401ece, false), (0x401f0e, true), (0x401f43, false)],
    gold: [
        (0x401670, "8b562c8b4c24442bd0515083ea246a2c52", 14),
        (0x4016d3, "8b4e2c8b54243883c4042bc8525083e9286a2851", 17),
        (0x401705, "8b4e2c6a01508b4424386a31", 10),
        (0x40174f, "8b4e2c6a01508b4424386a32", 10),
        (0x40180e, "8b4e2c5255992bc26a34d1f8", 8),
        (0x40183e, "8b4e2c6a02508b4424386a31", 10),
        (0x401888, "8b4e2c8bb6640200006a02508b4424386a32", 16),
    ],
    rect: 0x4020a6,
    rect_x: [0x4020b4, 0x402145],
    width_tests: [
        (0x401471, "3d00050000b920a86d007411", "3d00040000b920a86d007711"),
        (0x429adb, "3d000500000f85ff010000", "3d00050000909090909090"),
        (0x429ffc, "3d000500007545", "3d000500009090"),
    ],
};

/// Layout limits: width > 1024 (800 / 1024 have their own layouts), height >= 720 (side menu ends at y = 600), even
/// width, at most 4096x2400.
fn valid(w: u32, h: u32) -> bool {
    w > 1024 && h >= 720 && w <= 4096 && h <= 2400 && w % 2 == 0
}

pub struct Preset {
    pub w: u32,
    pub h: u32,
    /// screenGame.ini ANIM of the top bar (per width) / of the carving (per preset).
    pub anim_top: u32,
    pub anim_carv: u32,
    /// accelMap.ini SCREEN / ANIM of the overview map.
    pub accel: u32,
    /// textures.ini TEX of the top bar (per width) / of the carving (per height).
    pub tex_top: u32,
    pub tex_carv: u32,
    /// Overview map texture (original 1280x1024, or 1024x768 if it does not fit), centred.
    pub map_tex: u32,
    pub map_w: u32,
    pub map_h: u32,
    pub map_x: u32,
    pub map_y: u32,
}

pub struct Layout {
    /// Sorted by width, then height.
    pub presets: Vec<Preset>,
    pub widths: Vec<u32>,
    pub heights: Vec<u32>,
    /// Preset of this start.
    pub start: usize,
}

static LAYOUT: Cell<Layout> = Cell::new();
/// Address of PIDX (preset index chosen in the options) in the hook data.
static PIDX_VA: AtomicU32 = AtomicU32::new(0);

/// Ids: textures 101.. (top bars per width), 201.. (carvings per height); screenGame anims `anim0`.. (top bars, then
/// carvings per preset); accelMap screens 3...
fn layout(want: (u32, u32), anim0: u32) -> Layout {
    let mut sizes: Vec<(u32, u32)> = LIST.to_vec();
    if !sizes.contains(&want) {
        sizes.push(want);
    }
    sizes.sort_unstable();
    let mut widths: Vec<u32> = sizes.iter().map(|s| s.0).collect();
    widths.dedup();
    let mut heights: Vec<u32> = sizes.iter().map(|s| s.1).collect();
    heights.sort_unstable();
    heights.dedup();
    let wi = |w: u32| widths.iter().position(|&x| x == w).unwrap_or(0) as u32;
    let hi = |h: u32| heights.iter().position(|&x| x == h).unwrap_or(0) as u32;
    let presets = sizes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| {
            let (map_tex, map_w, map_h) = if w >= 1280 && h >= 1024 { (30024, 1280, 1024) } else { (30023, 1024, 768) };
            Preset {
                w,
                h,
                anim_top: anim0 + wi(w),
                anim_carv: anim0 + widths.len() as u32 + i as u32,
                accel: 3 + i as u32,
                tex_top: 101 + wi(w),
                tex_carv: 201 + hi(h),
                map_tex,
                map_w,
                map_h,
                map_x: (w - map_w) / 2,
                map_y: (h - map_h) / 2,
            }
        })
        .collect::<Vec<_>>();
    let start = sizes.iter().position(|&s| s == want).unwrap_or(0);
    Layout { presets, widths, heights, start }
}

pub(crate) fn current_layout() -> &'static Layout {
    LAYOUT.get()
}

/// `[display] width / height` of the settings: 0 / missing = [`DEFAULT`], invalid = [`DEFAULT`] (logged).
fn wanted() -> (u32, u32) {
    let ini = Ini::load(paths::settings().as_deref());
    let num = |k: &str| ini.get("display", k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (w, h) = (num("width"), num("height"));
    if w == 0 && h == 0 {
        return DEFAULT;
    }
    if valid(w, h) {
        return (w, h);
    }
    log::write(
        "qol",
        &format!("widescreen: [display] {w}x{h} out of limits (width 1026..4096 even, height 720..2400), using 1280x1024"),
    );
    DEFAULT
}

pub fn widescreen(p: &mut Plan, base: Base) -> Result<(), String> {
    let ad = match base {
        Base::P2 => &P2,
        Base::P3 => &P3,
    };
    let l = LAYOUT.set(layout(wanted(), ad.anim0));
    data::install(p, base)?;
    let np = l.presets.len() as u32;
    let start = &l.presets[l.start];
    let label = format!("{} x {}", start.w, start.h);

    let block = pcore::mem::alloc_code(0x2000)?;
    let mut a = Asm::new(block);
    // --- data ---
    let g = a.at();
    entry(&mut a, start);
    let (gw, gh, gtop, gcarv, gscr) = (g, g + 4, g + 8, g + 12, g + 16);
    let (gsx, gsy, gox, goy, grx, gty) = (g + 20, g + 28, g + 36, g + 44, g + 52, g + 56);
    let table = a.at();
    for pr in &l.presets {
        entry(&mut a, pr);
    }
    a.d32(0);
    let fmt = a.str("%d x %d");
    let pidx = a.at();
    a.d32(l.start as u32);
    let pbak = a.at();
    a.d32(l.start as u32);
    let ltop = a.at();
    a.d32(u32::MAX);
    let lcarv = a.at();
    a.d32(u32::MAX);
    let sgini = a.str("./scripts/ScreenGame.ini");

    // SETP: eax = preset index -> PIDX, options label "W x H" (all registers kept)
    let setp = a.at();
    a.b(&[0x60, 0xa3]); // pushad; mov [PIDX],eax
    a.d32(pidx);
    a.b(&[0x6b, 0xf0, ESZ as u8, 0x81, 0xc6]); // imul esi,eax,ESZ; add esi,TABLE
    a.d32(table);
    a.b(&[0xff, 0x76, 0x04, 0xff, 0x36, 0x68]); // push [esi+4]; push [esi]; push FMT
    a.d32(fmt);
    a.b(&[0x68]); // push LABEL
    a.d32(ad.label);
    a.b(&[0xff, 0x15]); // call [wsprintfA]
    a.d32(ad.wsprintfa);
    a.b(&[0x83, 0xc4, 0x10, 0x61, 0xc3]); // add esp,10h; popad; ret

    // COPYG: G = preset PIDX (all registers kept)
    let copyg = a.at();
    a.b(&[0x60, 0xa1]); // pushad; mov eax,[PIDX]
    a.d32(pidx);
    a.b(&[0x6b, 0xf0, ESZ as u8, 0x81, 0xc6]); // imul esi,eax,ESZ; add esi,TABLE
    a.d32(table);
    a.b(&[0xbf]); // mov edi,G
    a.d32(g);
    a.b(&[0xb9]); // mov ecx,ESZ/4
    a.d32(ESZ / 4);
    a.b(&[0xf3, 0xa5, 0x61, 0xc3]); // rep movsd; popad; ret

    // --- options list (resolution helper object, ecx; [ecx+4] = resolution index 0..2) ---
    let orig_np = hex("8b410456be01000000"); // mov eax,[ecx+4]; push esi; mov esi,1
    let thn = a.at();
    a.b(&orig_np);
    a.jmp(ad.next + 9);
    let thp = a.at();
    a.b(&orig_np);
    a.jmp(ad.prev + 9);
    // "next": at index 2 -> next preset; else original, and arriving at 2 starts at the smallest preset
    let next = a.at();
    a.b(&[0x83, 0x79, 0x04, 0x02]); // cmp dword [ecx+4],2
    a.j8(0x75, "NO");
    a.b(&[0xa1]); // mov eax,[PIDX]
    a.d32(pidx);
    a.b(&[0x83, 0xf8, (np - 1) as u8]); // cmp eax,NP-1
    a.j8(0x7d, "R2");
    a.b(&[0x40]); // inc eax
    a.call(setp);
    a.lab("RF");
    a.b(&[0x51]); // push ecx
    a.call(ad.buttons); // button enables
    a.b(&[0x59]); // pop ecx
    a.lab("R2");
    a.b(&[0xb8]); // mov eax,2
    a.d32(2);
    a.b(&[0xc3]);
    a.lab("NO");
    a.b(&[0xff, 0x71, 0x04, 0x51]); // push [ecx+4]; push ecx
    a.call(thn);
    a.b(&[0x59, 0x5a, 0x83, 0xfa, 0x02]); // pop ecx; pop edx; cmp edx,2 (old index)
    a.j8(0x74, "NR");
    a.b(&[0x83, 0x79, 0x04, 0x02]); // cmp dword [ecx+4],2
    a.j8(0x75, "NR");
    a.b(&[0x33, 0xc0]); // xor eax,eax
    a.call(setp);
    a.j8(0xeb, "RF");
    a.lab("NR");
    a.b(&[0xc3]);
    // "previous": at index 2 with a preset above the smallest -> previous preset; else original
    let prev = a.at();
    a.b(&[0x83, 0x79, 0x04, 0x02]);
    a.j8(0x75, "PO");
    a.b(&[0xa1]);
    a.d32(pidx);
    a.b(&[0x85, 0xc0]); // test eax,eax
    a.j8(0x7e, "PO");
    a.b(&[0x48]); // dec eax
    a.call(setp);
    a.j8(0xeb, "RF");
    a.lab("PO");
    a.jmp(thp);
    jump(p, ad.next, &orig_np, next)?;
    jump(p, ad.prev, &orig_np, prev)?;

    // button enables (bytes [esp+10h..13h]): at index 2 "+" ([esp+11h]) = not the last preset
    let cave = a.at();
    a.b(&[0x83, 0x79, 0x04, 0x02]);
    a.j8(0x75, "BE");
    a.b(&[0x83, 0x3d]); // cmp dword [PIDX],NP-1
    a.d32(pidx);
    a.b(&[(np - 1) as u8, 0x0f, 0x9c, 0x44, 0x24, 0x11]); // setl byte [esp+11h]
    a.lab("BE");
    a.b(&hex("33f68d790c")); // xor esi,esi; lea edi,[ecx+0Ch]
    a.jmp(ad.buttons_site + 5);
    jump(p, ad.buttons_site, &hex("33f68d790c"), cave)?;

    // options save (after the resolution write): also [display] width / height to settings.ini
    PIDX_VA.store(pidx, Ordering::Relaxed);
    let save: &'static [u8] = hex(ad.save_orig).leak();
    p.mid(ad.save, save.len(), save, &[Ins::Bytes(save)], &[], on_save, 0)?;

    // options opened (backs up the settings) / reset to the backup: preset too
    let orig = hex(ad.opened_orig);
    let cave = a.at();
    a.b(&orig);
    a.b(&[0x50, 0xa1]); // push eax; mov eax,[PIDX]
    a.d32(pidx);
    a.b(&[0xa3]); // mov [PBAK],eax
    a.d32(pbak);
    a.b(&[0x58]); // pop eax (used right after)
    a.jmp(ad.opened + 6);
    jump(p, ad.opened, &orig, cave)?;
    let orig = hex(ad.reset_orig);
    let cave = a.at();
    a.b(&[0x50, 0xa1]); // push eax (depth index); mov eax,[PBAK]
    a.d32(pbak);
    a.call(setp);
    a.b(&[0x58]); // pop eax
    a.b(&orig);
    a.jmp(ad.reset + 6);
    jump(p, ad.reset, &orig, cave)?;

    // mode switch sites: mov dword [esp+X],500h ; mov dword [esp+Y],400h -> [GW], [GH]
    for (va, x, y) in ad.modes {
        let orig = cat(&[&[0xc7, 0x44, 0x24, x], &le(1280), &[0xc7, 0x44, 0x24, y], &le(1024)]);
        let cave = a.at();
        a.call(copyg);
        a.b(&[0x50, 0xa1]); // push eax; mov eax,[GW]
        a.d32(gw);
        a.b(&[0x89, 0x44, 0x24, x + 4, 0xa1]); // mov [esp+X+4],eax; mov eax,[GH]
        a.d32(gh);
        a.b(&[0x89, 0x44, 0x24, y + 4, 0x58]); // mov [esp+Y+4],eax; pop eax
        a.jmp(va + 16);
        jump(p, va, &orig, cave)?;
    }
    // screen build: top bar / carving anim id
    for (s, cur, loaded, sync) in [(&ad.top, gtop, ltop, true), (&ad.carv, gcarv, lcarv, false)] {
        let (pre, post) = (hex(s.pre), hex(s.post));
        let mut orig = cat(&[&pre, &[0x89, 0x45, s.disp], &post]);
        if s.call != 0 {
            orig.extend_from_slice(&call_bytes(s.va + orig.len() as u32, s.call));
        }
        let cave = a.at();
        if sync {
            a.call(copyg);
        }
        a.b(&pre);
        a.b(&[0xa1]); // mov eax,[cur]
        a.d32(cur);
        a.b(&[0xa3]); // mov [loaded],eax
        a.d32(loaded);
        a.b(&[0x89, 0x45, s.disp]); // mov [ebp+disp],eax
        a.b(&post);
        if s.call != 0 {
            a.call(s.call);
        }
        a.jmp(s.va + orig.len() as u32);
        jump(p, s.va, &orig, cave)?;
    }

    // game screen layout (runs after every mode switch, ecx = game screen): the 1280 layout takes its widths from the
    // top bar picture and the carving is per height; both are loaded once at the screen build. If the preset changed
    // since, re-init them with the anims of the current preset (element vt+8 = init(CString ini, anim id) frees the
    // old frames first).
    let cave = a.at();
    a.b(&[0x60, 0x8b, 0xf1]); // pushad; mov esi,ecx
    for (cur, loaded, el, n) in [(gtop, ltop, ad.el_top, "XT"), (gcarv, lcarv, ad.el_carv, "XC")] {
        a.b(&[0xa1]); // mov eax,[loaded]; cmp eax,-1 (screen not built yet); je
        a.d32(loaded);
        a.b(&[0x83, 0xf8, 0xff]);
        a.j8(0x74, n);
        a.b(&[0xa1]); // mov eax,[cur]; cmp eax,[loaded]; je
        a.d32(cur);
        a.b(&[0x3b, 0x05]);
        a.d32(loaded);
        a.j8(0x74, n);
        a.b(&[0xa3]); // mov [loaded],eax
        a.d32(loaded);
        a.b(&[0x50, 0x51, 0x8b, 0xcc, 0x68]); // push id; push ecx; mov ecx,esp; push ini
        a.d32(sgini);
        a.call(ad.cstring); // CString(ini) in place
        a.b(&[0x8d, 0x8e]); // lea ecx,[esi+el]; mov eax,[ecx]; call [eax+8]
        a.d32(el);
        a.b(&[0x8b, 0x01, 0xff, 0x50, 0x08]);
        a.lab(n);
    }
    a.b(&hex("61 51 53 55 56 8b f1")); // popad; original prologue
    a.jmp(ad.layout + 6);
    jump(p, ad.layout, &hex("515355568bf1"), cave)?;
    // overview map screen
    let cave = a.at();
    a.b(&[0xff, 0x35]); // push [GSCR]
    a.d32(gscr);
    a.b(&[0x51, 0x8b, 0xcc]); // push ecx; mov ecx,esp
    a.jmp(ad.accel + 5);
    jump(p, ad.accel, &hex("6a02518bcc"), cave)?;

    // fast-travel overview map: ship/town positions = world (2400x1800) * scale, scale = screen size / world size,
    // drawn from the screen origin. In wide modes (width > 1024) the map is centred at its own size: scale = map size
    // / world, + map offset.
    let scrw = ad.scrw;
    let wide = |a: &mut Asm| {
        a.b(&[0x81, 0x3d]); // cmp dword [screen width],400h
        a.d32(scrw);
        a.d32(0x400);
    };
    let (sx, sy) = (ad.scale_x, ad.scale_x + 8);
    let fstp_y = cat(&[&[0xdd, 0x99], &le(sy)]);
    let cave = a.at();
    a.b(&fstp_y); // fstp qword [ecx+y] (y scale)
    wide(&mut a);
    a.j8(0x76, "SC"); // jbe
    a.b(&[0xdd, 0x05]); // fld [GSX]; fstp [ecx+x]
    a.d32(gsx);
    a.b(&[0xdd, 0x99]);
    a.d32(sx);
    a.b(&[0xdd, 0x05]); // fld [GSY]; fstp [ecx+y]
    a.d32(gsy);
    a.b(&fstp_y);
    a.lab("SC");
    a.jmp(ad.scale_site + 6);
    jump(p, ad.scale_site, &fstp_y, cave)?;
    // fmul qword [esi+x] / [esi+y] -> + offset
    for (va, x) in ad.fmul {
        let orig = cat(&[&[0xdc, 0x8e], &le(if x { sx } else { sy })]);
        let n = format!("F{va:x}");
        let cave = a.at();
        a.b(&orig);
        wide(&mut a);
        a.j8(0x76, &n);
        a.b(&[0xdc, 0x05]); // fadd qword [GOX/GOY]
        a.d32(if x { gox } else { goy });
        a.lab(&n);
        a.jmp(va + 6);
        jump(p, va, &orig, cave)?;
    }
    // gold/date box of the overview map: x = screen width - n, y = fixed (28h..34h) -> relative to the map's right /
    // top edge. Each run starts with "mov r32,[esi+2Ch]" and has one "push y"; both get wide-aware versions.
    for (va, run, yo) in ad.gold {
        let orig = hex(run);
        if orig.get(yo) != Some(&0x6a) || !orig.get(yo + 1).is_some_and(|y| (0x28..=0x34).contains(y)) {
            return Err(format!("gold box run at {va:x}: no push y"));
        }
        let n = format!("B{va:x}");
        let cave = a.at();
        a.b(&orig[..3]); // mov r32,[esi+2Ch]
        wide(&mut a);
        a.j8(0x76, &format!("{n}x"));
        a.b(&[0x8b, 0x05 | (orig[1] & 0x38)]); // mov r32,[GRX]
        a.d32(grx);
        a.lab(&format!("{n}x"));
        a.b(&orig[3..yo + 2]); // ... push y
        wide(&mut a);
        a.j8(0x76, &format!("{n}y"));
        a.b(&[0x50, 0xa1]); // push eax; mov eax,[GTY]; add [esp+4],eax; pop eax
        a.d32(gty);
        a.b(&[0x01, 0x44, 0x24, 0x04, 0x58]);
        a.lab(&format!("{n}y"));
        a.b(&orig[yo + 2..]);
        a.jmp(va + orig.len() as u32);
        jump(p, va, &orig, cave)?;
    }
    // ...and its redraw rect: left/right = screen width - 12Ch / - 28h, top/bottom = 28h / 4Bh -> map edges as above
    let cave = a.at();
    a.b(&hex("bd28000000bf4b000000")); // mov ebp,28h; mov edi,4Bh
    wide(&mut a);
    a.j8(0x76, "RT");
    a.b(&[0x03, 0x2d]); // add ebp,[GTY]
    a.d32(gty);
    a.b(&[0x03, 0x3d]); // add edi,[GTY]
    a.d32(gty);
    a.lab("RT");
    a.b(&[0x3b, 0xd0]); // cmp edx,eax (flags for the je after)
    a.jmp(ad.rect + 12);
    jump(p, ad.rect, &hex("bd280000003bd0bf4b000000"), cave)?;
    for va in ad.rect_x {
        let n = format!("RX{va:x}");
        let cave = a.at();
        a.b(&[0x8b, 0x46, 0x2c]); // mov eax,[esi+2Ch]
        wide(&mut a);
        a.j8(0x76, &n);
        a.b(&[0xa1]); // mov eax,[GRX]
        a.d32(grx);
        a.lab(&n);
        a.b(&[0x8d, 0x54, 0x24, 0x44]); // lea edx,[esp+44h]
        a.jmp(va + 7);
        jump(p, va, &hex("8b462c8d542444"), cave)?;
    }
    // width tests "== 1280" -> "> 1024"
    for (va, orig, new) in ad.width_tests {
        p.patch(va, &hex(orig), &hex(new))?;
    }
    // options menu label (11 characters + NUL, "Invert stereo" follows)
    let mut lab = label.into_bytes();
    lab.resize(12, 0);
    p.patch(ad.label, &hex("313238302078203130323400"), &lab)?;

    let code = a.finish()?;
    if code.len() > 0x2000 {
        return Err("widescreen code block too long".into());
    }
    unsafe { core::ptr::copy_nonoverlapping(code.as_ptr(), block as *mut u8, code.len()) };
    log::write(
        "qol",
        &format!("widescreen: {} presets, resolution 3 = {}x{} (cache mods\\qol\\wide)", np, start.w, start.h),
    );
    Ok(())
}

fn entry(a: &mut Asm, p: &Preset) {
    for v in [p.w, p.h, p.anim_top, p.anim_carv, p.accel] {
        a.d32(v);
    }
    for v in [p.map_w as f64 / 2400.0, p.map_h as f64 / 1800.0, p.map_x as f64, p.map_y as f64] {
        a.b(&v.to_le_bytes());
    }
    a.d32(p.map_x + p.map_w);
    a.d32(p.map_y);
}

/// `jmp to` over the whole site (checked against `expect`), rest nops.
fn jump(p: &mut Plan, va: u32, expect: &[u8], to: u32) -> Result<(), String> {
    let mut b = rel5(0xe9, va, to).to_vec();
    b.resize(expect.len(), 0x90);
    p.patch(va, expect, &b)
}

/// Options saved: `[display] width / height` = preset PIDX.
unsafe extern "C" fn on_save(_ctx: *mut Ctx, _: u32) {
    let l = LAYOUT.get();
    let Some(pr) = l.presets.get(rd32(PIDX_VA.load(Ordering::Relaxed)) as usize) else { return };
    let Some(file) = paths::settings() else { return };
    let w = |s: &str| pcore::wide(s.as_ref());
    let f = pcore::wide(file.as_os_str());
    let sec = w("display");
    for (k, v) in [("width", pr.w), ("height", pr.h)] {
        pcore::ffi::WritePrivateProfileStringW(sec.as_ptr(), w(k).as_ptr(), w(&v.to_string()).as_ptr(), f.as_ptr());
    }
}

/// Code emitter for one block (data + code) at a fixed address; short jumps by label.
struct Asm {
    base: u32,
    c: Vec<u8>,
    labels: HashMap<String, u32>,
    fix8: Vec<(usize, String)>,
}

impl Asm {
    fn new(base: u32) -> Asm {
        Asm { base, c: Vec::new(), labels: HashMap::new(), fix8: Vec::new() }
    }

    fn at(&self) -> u32 {
        self.base + self.c.len() as u32
    }

    fn b(&mut self, x: &[u8]) {
        self.c.extend_from_slice(x);
    }

    fn d32(&mut self, v: u32) {
        self.b(&le(v));
    }

    fn rel32(&mut self, to: u32) {
        let next = self.at() + 4;
        self.d32(to.wrapping_sub(next));
    }

    fn jmp(&mut self, to: u32) {
        self.b(&[0xe9]);
        self.rel32(to);
    }

    fn call(&mut self, to: u32) {
        self.b(&[0xe8]);
        self.rel32(to);
    }

    /// NUL-terminated string, then aligned to 4; returns its address.
    fn str(&mut self, s: &str) -> u32 {
        let at = self.at();
        self.b(s.as_bytes());
        self.b(&[0]);
        while self.c.len() % 4 != 0 {
            self.c.push(0);
        }
        at
    }

    fn lab(&mut self, n: &str) {
        let at = self.at();
        self.labels.insert(n.to_string(), at);
    }

    fn j8(&mut self, op: u8, n: &str) {
        self.b(&[op, 0]);
        self.fix8.push((self.c.len() - 1, n.to_string()));
    }

    fn finish(mut self) -> Result<Vec<u8>, String> {
        for (pos, n) in &self.fix8 {
            let to = *self.labels.get(n).ok_or_else(|| format!("widescreen: label {n} missing"))?;
            let d = to as i64 - (self.base as i64 + *pos as i64 + 1);
            if !(-128..=127).contains(&d) {
                return Err(format!("widescreen: short jump to {n} out of range"));
            }
            self.c[*pos] = d as i8 as u8;
        }
        Ok(self.c)
    }
}
