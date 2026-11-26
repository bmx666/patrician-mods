//! `Win32\Mp3dec.asi`: Miles Sound System 6 MPEG audio ASI codec ("*.MP3").
//!
//! The original `Mp3dec.asi` (MSS 6.0J) decodes with x87 only; its fast path is AMD 3DNow!. Rosetta 2 emulates
//! x87 slowly, so every music change froze the game. This codec uses minimp3 (CC0) built for SSE2, which Rosetta
//! translates to native code. The interface copies the original ([`asi`]).

use asi::{Codec, Frame, Stats, Stream};

/// minimp3 `mp3dec_t`.
#[repr(C)]
#[allow(dead_code)]
struct Mp3Dec {
    mdct_overlap: [[f32; 9 * 32]; 2],
    qmf_state: [f32; 15 * 2 * 32],
    reserv: i32,
    free_format_bytes: i32,
    header: [u8; 4],
    reserv_buf: [u8; 511],
}

/// minimp3 `mp3dec_frame_info_t`.
#[repr(C)]
#[allow(dead_code)]
#[derive(Default)]
struct FrameInfo {
    frame_bytes: i32,
    frame_offset: i32,
    channels: i32,
    hz: i32,
    layer: i32,
    bitrate_kbps: i32,
}

/// `MINIMP3_MAX_SAMPLES_PER_FRAME`.
const MAX_SAMPLES: usize = 1152 * 2;

extern "C" {
    fn mp3dec_init(dec: *mut Mp3Dec);
    fn mp3dec_decode_frame(dec: *mut Mp3Dec, mp3: *const u8, bytes: i32, pcm: *mut i16, info: *mut FrameInfo) -> i32;
    fn mp3_frame_len(h: *const u8) -> i32;
}

struct Mp3 {
    dec: Box<Mp3Dec>,
    out: Box<[i16; MAX_SAMPLES]>,
    out_bytes: usize,
}

impl Codec for Mp3 {
    const NAME: &'static [u8] = b"MSS MPEG Layer 3 Audio Decoder (SSE2)\0";
    const INPUT_TYPES: &'static [u8] = b"MPEG Layer 3 audio files\0*.MP3\0\0";
    const INBUF: usize = 16384;
    const BLOCK: i32 = 0x1000;
    const HEADER: usize = 4;
    const NO_DATA: &'static [u8] = b"No MPEG audio data\0";
    const EXTRA: &'static [(&'static [u8], u32)] = &[(b"MPEG version\0", 14), (b"MPEG layer\0", 15)];

    fn new() -> Option<Self> {
        // SAFETY: mp3dec_t is plain data; mp3dec_init sets what it needs
        let mut dec: Box<Mp3Dec> = Box::new(unsafe { core::mem::zeroed() });
        unsafe { mp3dec_init(&mut *dec) };
        Some(Mp3 { dec, out: Box::new([0; MAX_SAMPLES]), out_bytes: 0 })
    }

    fn frame_len(h: &[u8]) -> usize {
        unsafe { mp3_frame_len(h.as_ptr()) }.max(0) as usize
    }

    fn decode(&mut self, frame: &[u8]) -> Result<Frame, usize> {
        // exactly one frame: minimp3 accepts it without seeing the next header
        let mut fi = FrameInfo::default();
        let n = unsafe {
            mp3dec_decode_frame(&mut *self.dec, frame.as_ptr(), frame.len() as i32, self.out.as_mut_ptr(), &mut fi)
        };
        if fi.frame_bytes <= 0 {
            return Err(1); // false sync
        }
        self.out_bytes = (n.max(0) * fi.channels.max(0) * 2) as usize;
        Ok(Frame {
            used: fi.frame_bytes as usize,
            pcm_bytes: self.out_bytes,
            rate: fi.hz,
            channels: fi.channels,
            samples: n.max(0) as u32,
            kbps: fi.bitrate_kbps,
            layer: fi.layer,
        })
    }

    fn pcm(&self) -> &[u8] {
        // SAFETY: i16 array viewed as bytes
        unsafe { core::slice::from_raw_parts(self.out.as_ptr().cast(), MAX_SAMPLES * 2) }
    }

    fn reset(&mut self) {
        unsafe { mp3dec_init(&mut *self.dec) };
    }

    fn bitrate(s: &Stats) -> i32 {
        if s.kbps > 0 { s.kbps * 1000 } else { 128_000 }
    }

    fn attribute(s: &Stream<Self>, token: u32) -> i32 {
        match token {
            14 => 1,
            15 if s.layer > 0 => s.layer,
            15 => 3,
            _ => -1,
        }
    }
}

asi::dll_main!(Mp3);
