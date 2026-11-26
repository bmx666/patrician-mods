# Patrician mods

Mod loader and quality-of-life mod for **Patrician II** (GOG) and **Patrician III** (GOG, Steam). The game files are not modified: everything is applied in memory when the game starts.

> **Early test version.** The loader and `qol.dll` are new. Please report problems with your `mods\loader.log`.

## Contents

| File | Purpose |
|---|---|
| `winmm.dll` | Loads the mods when the game starts. Windows loads it automatically from the game folder. |
| `patrician-launcher.exe` | Alternative way to start the game with the mods, without `winmm.dll` (for Wine / macOS). |
| `mods\qol.dll` | Quality-of-life features and fixes of original bugs (see below). |
| `mods\localization.dll` | Language files from `mods\localization\<lang>\<game>\` (see below). |
| `mods\nosmc.dll` | Faster drawing on Apple Silicon and other ARM64 hosts (see below). |
| `mods\settings.ini` | Settings; every feature can be switched off. |
| `Win32\Mp3dec.asi` | MP3 decoder of the game's sound system, replaces the original (see below). |
| `Win32\Aacdec.asi` | AAC decoder for the game's sound system (see below). |
| `licenses\` | Licenses of the decoders minimp3 and FAAD3. |

The graphics renderer `ddraw_dll.dll` is not part of this package. Get it from [patrician-ddraw](https://github.com/bmx666/patrician-ddraw/releases) and replace the game's file yourself.

## Supported games

| Game | Executable | Status |
|---|---|---|
| Patrician II, GOG UK v1.1 | `Patrician 2.exe` | supported |
| Patrician III, GOG 1.1 | `Patrician3.exe` | supported |
| Patrician III, Steam | `Patrician3.exe` | not yet: the Steam executable decrypts its code only after start-up |

Other versions are detected and left alone: every feature checks the original bytes first and is skipped when they differ (written to `mods\loader.log`).

## Installation

1. Extract the zip into the game folder (next to `Patrician 2.exe` or `Patrician3.exe`).
2. Start the game as usual.
   - **Windows:** `winmm.dll` is picked up automatically.
   - **Wine, CrossOver, Game Porting Toolkit (macOS), Proton:** Wine prefers its own `winmm.dll`. Either start the game with `patrician-launcher.exe`, or set the DLL override `winmm` to *native, builtin* (`winecfg` → Libraries, or `WINEDLLOVERRIDES="winmm=n,b"`).

To uninstall, delete `winmm.dll`, `patrician-launcher.exe`, `Win32\Aacdec.asi`, the `mods` and `licenses` folders, and restore the original `Win32\Mp3dec.asi` from the game installer.

### P3Modding

The loader uses the same mod interface as the [P3Modding modloader](https://p3modding.github.io/modloader.html): every `mods\*.dll` is loaded and its `start` export is called. P3Modding mods therefore also load through `winmm.dll` / the launcher, and `qol.dll` also works under P3Modding. When the P3Modding modloader is running, `winmm.dll` leaves the loading to it.

## Features (`mods\qol.dll`)

| Key in `[qol]` | P2 | P3 | Feature |
|---|---|---|---|
| `music_fade` | ✓ | ✓ | Fix: the previous music track fades out within 5 s at any volume (it could keep playing for minutes when the music slider was low). |
| `tutorial_ambience` | ✓ | ✓ | Fix: building ambience no longer gets stuck during the tutorial. |
| `single_game_title` | ✓ | ✓ | Fix: the new single game screen shows "Single player" instead of "Game settings". |
| `shipyard_knots` | ✓ | ✓ | Fix: the shipyard shows the real base speed of each ship type (5.5 / 6 / 4.5 / 5 knots). |
| `campaign_speed` | ✓ | | Fix: a campaign starts with the game speed of the settings. |
| `ship_speed` | ✓ | ✓ | Ship panel: current speed in tenths of a knot. |
| `building_backdrop` | ✓ | | Building windows: more opaque backdrop behind the text. |
| `trade_route_dialog` | | ✓ | Trade route file dialog: wider window, large buttons. |
| `wasd` | ✓ | ✓ | W A S D scroll the town and sea map like the arrow keys. |
| `speed_keys` | ✓ | ✓ | `+` / `-` (main keyboard or numpad) change the game speed. |
| `speed_plate` | ✓ | ✓ | Game speed buttons centred below the top bar; click to set the speed. |
| `stream_open` | ✓ | ✓ | Music and voice files are played as `.mp3` or `.aac`, whichever exists. |
| `widescreen` | ✓ | ✓ | The third resolution of the options ("1280 x 1024") becomes any size, see below. |

### Sound decoders (`Win32\*.asi`)

Both games play music and voices through the Miles Sound System, which loads every decoder in `Win32\*.asi` and picks one by the file extension.

- `Mp3dec.asi` replaces the original MP3 decoder. The original runs on the x87 FPU only, which Rosetta 2 (macOS on Apple Silicon) emulates slowly, so the game froze on every music change. This one is [minimp3](https://github.com/lieff/minimp3) built for SSE2.
- `Aacdec.asi` adds `.aac` files (ADTS; AAC-LC, HE-AAC v1/v2, mono or stereo), decoded by FAAD3 from [faac](https://github.com/FreewareAdvancedAudio/faac). With `stream_open`, an `.aac` file is played wherever the game asks for the `.mp3` of the same name. An ID3v2 `TLEN` frame gives the exact track length.

Keys and speed changes are ignored while a window with a text field is open, and follow the multiplayer rules of the game (only the host or a single player changes the speed).

### Widescreen

The third resolution of the options uses the size from `mods\settings.ini`:

```ini
[display]
width=1920      ; 0 = 1280 x 1024 (the original size)
height=1080
```

With this resolution selected, the options list also steps through 33 common sizes (4:3, 5:4, 16:10, 16:9, 21:9, 32:9, Mac Retina) and the chosen one is saved to `[display]`. Any other size works as well: width 1026 to 4096 (even), height 720 to 2400. The size changes when a game is started or loaded. The top bar, the carving at the bottom right and the fast-travel map are fitted to the size. Their pictures and ini entries are made from the game's own files at every start, in `mods\qol\wide\`.

## Language (`mods\localization.dll`)

The language is set in `mods\settings.ini`:

```ini
[localization]
lang=en         ; two-letter ISO 639-1 code; missing = en
```

Files in `mods\localization\<lang>\<game>\` replace the game files of the same name and relative path, for every file the game reads from its archives (pictures, ini, scripts, fonts). `<game>` is `p2` for Patrician II and `p3` for Patrician III; each game reads only its own folder. Example: `mods\localization\ru\p2\scripts\buttons.ini` replaces `scripts\buttons.ini` of Patrician II. Names are not case-sensitive. The folder is read once when the game starts. To get the original game back, delete the `mods` folder.

The main menu has a language selector at the top left: `<` / `>` step through the flags of `en` and every language folder in `mods\localization\` that has a folder of the running game (GB, DE, DK, NO, SE, FI, EE, LV, LT, RU; a folder without a known flag is listed in the log only), the check button below the flag writes `lang` to `mods\settings.ini` and restarts the game (grey = the running language). After `patrician-launcher.exe` the game restarts through the launcher.

Texts built into the executable (menus, messages, names; in Patrician III also the tool tips) come from the language's `exe\*.tsv`, with the font charset from `exe\charset.bin`. Hooks fit the grammar of the translation: forms that depend on a number, adjectives by gender, a capital letter for a name at the start of a sentence, and the decorated initial letters (drop caps) of national alphabets. This needs the GOG executable of either game; Patrician III Steam gets the replaced files only. With `lang=en` the executable is left as is. Every patched place is checked against the original bytes first; a feature whose bytes differ is skipped and written to `mods\loader.log`.

Patrician II: when the language's `scripts\buttons.ini` defines `[Button6]` (72x18), the Undo button of the automatic trading window uses it (room for longer words such as Отмена or Atšaukti), and the "trade with town / office" toggles move apart to make room.

Tutorial voice-over for the translated languages: record or synthesize it yourself, see [voice/README.md](voice/README.md).

## Apple Silicon / ARM64 (`mods\nosmc.dll`)

The image decoder `AIM.dll` of both games draws sprites with self-modifying code: its blitters rewrite their own instructions and keep counters inside their code pages on every sprite and row. An x86 translator (Rosetta 2 under Wine / CrossOver / Game Porting Toolkit on macOS, Prism on Windows on ARM, FEX or Box86 on Linux) throws its translation of a page away on every such write. `nosmc.dll` moves that data out of the code pages and makes the patched instructions read variables instead, in memory at start; `AIM.dll` on disk is not changed.

```ini
[nosmc]
enabled=auto    ; auto = on for macOS, Rosetta 2 or an ARM64 host; 1 = always; 0 = never
```

`mods\loader.log` shows the detected host (Wine version and host system, CPU name, native machine) and the result.

## Settings (`mods\settings.ini`)

```ini
[loader]
enabled=1       ; 0 = load no mods
exe=            ; launcher: game executable, empty = Patrician3.exe, else Patrician 2.exe

[qol]
wasd=1          ; one key per feature, see the table above; missing = 1

[display]
width=0         ; widescreen, see above
height=0

[localization]
lang=en         ; see Language above

[nosmc]
enabled=auto    ; see Apple Silicon / ARM64 above
```

## Log

`mods\loader.log` is rewritten at every start. It lists the detected game, every loaded mod and, for `qol.dll` and `localization.dll`, every feature as `on` or `skipped` with the reason.

## Building

Rust 1.88 or newer, the target `i686-pc-windows-gnu`, [zig](https://ziglang.org) and [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild). Works on macOS, Linux and Windows:

```sh
rustup target add i686-pc-windows-gnu
cargo install --locked cargo-zigbuild
cargo zigbuild --release
sh tools/package.sh          # dist/patrician-mods-<version>.zip
```

| Crate | Output |
|---|---|
| `crates/core` (`pcore`) | shared: game detection, settings, log, memory patches, hooks, mod loading |
| `crates/winmm` | `winmm.dll` (forwards every export to the system `winmm.dll`) |
| `crates/launcher` | `patrician-launcher.exe` |
| `crates/qol` | `qol.dll` |
| `crates/localization` | `localization.dll` |
| `crates/nosmc` | `nosmc.dll` (x86 decoder: [iced-x86](https://github.com/icedland/iced), MIT) |
| `crates/asi` | shared: Miles ASI codec registration and streaming |
| `crates/asi-mp3` | `mp3dec.dll` → `Win32\Mp3dec.asi` (minimp3, C, CC0) |
| `crates/asi-aac` | `aacdec.dll` → `Win32\Aacdec.asi` (FAAD3, C, LGPL-2.1-or-later) |

The GitHub Actions workflow `.github/workflows/build.yml` builds the zip; it runs only when started by hand.

## License

[MIT](LICENSE). The decoders compiled into the `.asi` files keep their licenses: minimp3 is CC0 (`crates/asi-mp3/c/LICENSE.minimp3`), FAAD3 is LGPL-2.1-or-later (`crates/asi-aac/faad3/COPYING`, source and version in `crates/asi-aac/faad3/UPSTREAM`). *Patrician II* and *Patrician III* are trademarks of their respective owners. This project is not affiliated with them.
