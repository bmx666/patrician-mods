//! Codecs of the `AIM.dll` replacement (`crates/aim`) that work on byte slices and run on any host, so that they
//! can be tested with `cargo test -p aimcodec --target <host>`:
//!
//! - [`jpeg`]: JPEG decoder (baseline, extended and progressive Huffman) and baseline encoder, in place of the Intel
//!   JPEG Library `ijl11.dll` that the original `AIM.dll` calls;
//! - [`lz`]: the LZ coder of the compressed chunks of AIM layers;
//! - [`dxt`]: DXT1..DXT5 compression (the original asks DirectDraw to do it).
#![forbid(unsafe_code)]

pub mod dxt;
pub mod jpeg;
pub mod lz;
