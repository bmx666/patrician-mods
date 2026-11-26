//! `Win32\Aacdec.asi`: Miles Sound System 6 ASI codec for AAC in ADTS streams ("*.AAC"), next to `Mp3dec.asi`.
//!
//! Decoder: FAAD3 (`faad3/`, LGPL-2.1-or-later), AAC-LC and HE-AAC v1/v2 (implicit SBR / PS), up to 2 channels.
//! Input is ADTS only (no MP4 / M4A, ADIF or LATM). The game opens `...\x.aac` streams through this provider with
//! no exe change ([`asi`]).

use asi::{Codec, Frame, Stats};
use core::ffi::c_void;

mod math;

/// FAAD3 `faad_config`.
#[repr(C)]
#[allow(dead_code)]
struct Config {
    struct_size: u32,
    stream_format: u32,
    output_format: u32,
    downmix_mode: u32,
    drc_cut: u8,
    drc_boost: u8,
    drc_target_level: f32,
}

/// FAAD3 `faad_stream_info`.
#[repr(C)]
#[allow(dead_code)]
#[derive(Default)]
struct Info {
    struct_size: u32,
    sample_rate: u32,
    channels: u32,
    object_type: u32,
    decoder_delay: u32,
    frame_samples: u32,
    max_output_bytes: u32,
    channel_mask: u32,
    flags: u32,
    drc_ref_level: f32,
}

impl Info {
    fn new() -> Self {
        Info { struct_size: core::mem::size_of::<Info>() as u32, ..Default::default() }
    }
}

const FAAD_OK: i32 = 0;

extern "C" {
    fn faad_config_init(config: *mut Config, size: u32) -> i32;
    fn faad_decoder_open(config: *const Config, asc: *const u8, asc_len: u32, out: *mut *mut c_void) -> i32;
    fn faad_decoder_close(dec: *mut *mut c_void) -> i32;
    fn faad_decoder_get_info(dec: *const c_void, info: *mut Info) -> i32;
    fn faad_decoder_flush(dec: *mut c_void) -> i32;
    fn faad_decode_frame(
        dec: *mut c_void,
        input: *const u8,
        in_bytes: u32,
        used: *mut u32,
        out: *mut c_void,
        out_cap: u32,
        wrote: *mut u32,
        flags: *mut u32,
    ) -> i32;
}

/// ADTS header without CRC.
const ADTS_HDR: usize = 7;
/// Input buffer, larger than the 13-bit ADTS frame_length limit.
const INBUF: usize = 8192;

struct Aac {
    dec: *mut c_void,
    out: Vec<u8>,
}

impl Drop for Aac {
    fn drop(&mut self) {
        unsafe { faad_decoder_close(&mut self.dec) };
    }
}

impl Codec for Aac {
    const NAME: &'static [u8] = b"MSS AAC Audio Decoder (FAAD3, SSE2)\0";
    const INPUT_TYPES: &'static [u8] = b"AAC audio files (ADTS)\0*.AAC\0\0";
    const INBUF: usize = INBUF;
    const BLOCK: i32 = INBUF as i32;
    const HEADER: usize = ADTS_HDR;
    const NO_DATA: &'static [u8] = b"No AAC (ADTS) audio data\0";

    fn new() -> Option<Self> {
        // SAFETY: faad_config is plain data; faad_config_init fills it
        let mut cfg: Config = unsafe { core::mem::zeroed() };
        let mut info = Info::new();
        let mut dec = Aac { dec: core::ptr::null_mut(), out: Vec::new() };
        unsafe {
            if faad_config_init(&mut cfg, core::mem::size_of::<Config>() as u32) != FAAD_OK
                || faad_decoder_open(&cfg, core::ptr::null(), 0, &mut dec.dec) != FAAD_OK
                || faad_decoder_get_info(dec.dec, &mut info) != FAAD_OK
                || info.max_output_bytes == 0
            {
                return None;
            }
        }
        dec.out = vec![0; info.max_output_bytes as usize];
        Some(dec)
    }

    fn frame_len(h: &[u8]) -> usize {
        if h[0] != 0xFF || h[1] & 0xF6 != 0xF0 {
            return 0; // syncword, layer 0
        }
        if (h[2] >> 2) & 15 > 12 {
            return 0; // sampling_frequency_index
        }
        let n = usize::from(h[3] & 3) << 11 | usize::from(h[4]) << 3 | usize::from(h[5] >> 5);
        let hdr = if h[1] & 1 != 0 { ADTS_HDR } else { ADTS_HDR + 2 }; // protection_absent = 0: CRC
        if n > hdr && n <= INBUF { n } else { 0 }
    }

    fn decode(&mut self, frame: &[u8]) -> Result<Frame, usize> {
        // exactly one frame: FAAD3 decodes it without seeing the next header
        let (mut used, mut wrote, mut flags) = (0u32, 0u32, 0u32);
        let st = unsafe {
            faad_decode_frame(
                self.dec,
                frame.as_ptr(),
                frame.len() as u32,
                &mut used,
                self.out.as_mut_ptr().cast(),
                self.out.len() as u32,
                &mut wrote,
                &mut flags,
            )
        };
        if st != FAAD_OK {
            return Err(used as usize); // false sync / rejected
        }
        let mut info = Info::new();
        let (rate, channels, samples) = if wrote > 0 && unsafe { faad_decoder_get_info(self.dec, &mut info) } == FAAD_OK {
            (info.sample_rate as i32, info.channels as i32, if info.frame_samples > 0 { info.frame_samples } else { 1024 })
        } else {
            (0, 0, 0)
        };
        Ok(Frame { used: used as usize, pcm_bytes: wrote as usize, rate, channels, samples, kbps: 0, layer: 0 })
    }

    fn pcm(&self) -> &[u8] {
        &self.out
    }

    fn reset(&mut self) {
        unsafe { faad_decoder_flush(self.dec) };
    }

    /// Miles reads it once at open: bytes/s = rate / 8, stream length in ms = file bytes * 1000 / (bytes/s) (Mss32
    /// 2111D1F6, 2111E2C0; `AIL_stream_ms_position`). The fetch callback only sees the current Miles buffer, so the
    /// file cannot be scanned: the exact length comes from the ID3v2 TLEN frame. Without it: nominal 64 kbit/s (the
    /// first frames are encoder priming, far below the average), after open the average of the frames decoded so far.
    fn bitrate(s: &Stats) -> i32 {
        if s.len_ms > 0 && s.total > 0 {
            (s.total as u64 * 8000 / s.len_ms as u64) as i32
        } else if s.frame_samples >= 32 * 1024 && s.rate > 0 {
            (s.frame_bytes * 8 * s.rate as u64 / s.frame_samples) as i32
        } else {
            64_000
        }
    }
}

asi::dll_main!(Aac);
