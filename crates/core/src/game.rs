//! Supported executables, told apart by PE TimeDateStamp + SizeOfImage.
use crate::pe::PeInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    /// Patrician II, GOG UK v1.1 (`Patrician 2.exe`).
    P2Gog,
    /// Patrician III, GOG 1.1 w10 (`Patrician3.exe`).
    P3Gog,
    /// Patrician III, Steam: same build as GOG inside a SteamStub wrapper; the code is decrypted only after the stub
    /// runs, so patches work only when applied later (P3Modding calls `start` at WinMain).
    P3Steam,
}

/// Code base of a game: the patch addresses are the same for P3 GOG and Steam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    P2,
    P3,
}

impl Game {
    pub fn identify(info: &PeInfo) -> Option<Game> {
        match (info.timestamp, info.size_of_image) {
            (0x3f1f_b3a2, 0x003a_9000) => Some(Game::P2Gog),
            (0x3f54_4d3c, 0x002f_9000) => Some(Game::P3Gog),
            (0x3f54_4d3c, 0x0034_f000) => Some(Game::P3Steam),
            _ => None,
        }
    }

    pub fn base(self) -> Base {
        match self {
            Game::P2Gog => Base::P2,
            Game::P3Gog | Game::P3Steam => Base::P3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Game::P2Gog => "Patrician II (GOG)",
            Game::P3Gog => "Patrician III (GOG)",
            Game::P3Steam => "Patrician III (Steam)",
        }
    }
}

/// Executable names looked up by the launcher, in this order.
pub const EXE_NAMES: [&str; 2] = ["Patrician3.exe", "Patrician 2.exe"];
