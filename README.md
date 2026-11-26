# Patrician mods

Mod loader and quality-of-life mod for **Patrician II** (GOG) and **Patrician III** (GOG, Steam). The game files are not modified: everything is applied in memory when the game starts.

> **Early test version.** The loader and `qol.dll` are new. Please report problems with your `mods\loader.log`.

## Contents

| File | Purpose |
|---|---|
| `winmm.dll` | Loads the mods when the game starts. Windows loads it automatically from the game folder. |
| `patrician-launcher.exe` | Alternative way to start the game with the mods, without `winmm.dll` (for Wine / macOS). |
| `mods\qol.dll` | Quality-of-life features and fixes of original bugs (see below). |
| `mods\localization.dll` | Language files from `mods\localization\<lang>\` (see below). |
| `mods\settings.ini` | Settings; every feature can be switched off. |

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

To uninstall, delete `winmm.dll`, `patrician-launcher.exe` and the `mods` folder.

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
| `widescreen` | ✓ | | The third resolution of the options ("1280 x 1024") becomes any size, see below. |

Keys and speed changes are ignored while a window with a text field is open, and follow the multiplayer rules of the game (only the host or a single player changes the speed).

### Widescreen (Patrician II)

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

Files in `mods\localization\<lang>\` replace the game files of the same name and relative path, for both games and every file the game reads from its archives (pictures, ini, scripts, fonts). Example: `mods\localization\ru\scripts\buttons.ini` replaces `scripts\buttons.ini`. Names are not case-sensitive. The folder is read once when the game starts. To get the original game back, delete the `mods` folder.

Patrician II: when the language's `scripts\buttons.ini` defines `[Button6]` (72x18), the Undo button of the automatic trading window uses it (room for longer words such as Отмена or Atšaukti), and the "trade with town / office" toggles move apart to make room.

## Settings (`mods\settings.ini`)

```ini
[loader]
enabled=1       ; 0 = load no mods
exe=            ; launcher: game executable, empty = Patrician3.exe, else Patrician 2.exe

[qol]
wasd=1          ; one key per feature, see the table above; missing = 1

[display]
width=0         ; Patrician II widescreen, see above
height=0

[localization]
lang=en         ; see Language above
```

## Log

`mods\loader.log` is rewritten at every start. It lists the detected game, every loaded mod and, for `qol.dll`, every feature as `on` or `skipped` with the reason.

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

The GitHub Actions workflow `.github/workflows/build.yml` builds the zip; it runs only when started by hand.

## License

[MIT](LICENSE). *Patrician II* and *Patrician III* are trademarks of their respective owners. This project is not affiliated with them.
