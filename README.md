# reDraw

A tiny, pixel-sharp crosshair overlay for Windows. One small exe (about half a megabyte), always on
top, click-through, and every pixel is a whole pixel: nothing is ever blended or smoothed.

**It never reads game data.** No game memory, no handles to other processes, no hooks, no injection.
It only looks at your own mouse, the keys you choose, and (only for Auto contrast) a small patch of
screen behind the crosshair.

by christopher robin fiore

## Install

Download **`reDraw-Setup.exe`** from the [latest release](https://github.com/globalanomalyindex/redraw/releases/latest)
and run it. It installs for you only (no administrator rights) into `%LOCALAPPDATA%\reDraw`, adds a Start menu
shortcut and a Settings > Apps entry, and asks whether to start reDraw with Windows.

Or just download **`redraw.exe`** and double-click it; it keeps its settings (`redraw.cfg`) beside itself.

The files are not code-signed, so Windows may show "Windows protected your PC" the first time:
choose **More info**, then **Run anyway**. Games must be windowed or borderless; an exclusive-fullscreen game
can hide any overlay.

## Use it

| Key | |
| --- | --- |
| `F10` | open or close the menu (top-left corner) |
| `F9` | show or hide the crosshair |
| `F11` | reload `redraw.cfg` after editing it by hand |
| `Ctrl+Alt+F9` | quit |

These controls are also listed in white under the title in the menu. In the menu: click, right-click a row to go back,
arrows / Enter / Space / Esc, mouse wheel to scroll. At the very bottom, **Restart** (orange) restarts the program,
installing a newer version first if there is one, and **ForceQuit** (red) closes it at once.

- **Layers:** up to 8 of Spokes, Ring, Diamond, Bracket, Dot, Chevron, Polygon (and stars), Arc, Ladder, Line, Disc,
  each with its own color (sliders, hex, or a color cycle), opacity, shift and mirror.
- **Profiles:** a hip fire and an ADS crosshair, switched by holding or toggling right mouse, or a key, and
  crossfaded (Morph).
- **States:** bind crouch, sprint and jump keys to tighten, widen or recolor the crosshair while held.
- **Share codes:** copy your whole crosshair as one line of text, paste someone else's.
- **Experimental [!]:** Glow and lens blends (Invert, Contrast, Gray), scope zoom, **Aspect ratio** (a magnified area
  at the center that takes any shape from 1:1 to 21:9 or 9:16, and either stretches what it shows or crops it),
  input dynamics, spray trace, auto contrast. All of it works from Windows' own magnifier and the screen behind the
  crosshair; nothing touches the game.
- **Display:** the crosshair sits in the middle of your primary screen (or another monitor, or the focused window),
  scaled by whole numbers to suit any resolution and refresh rate.

## Updates

reDraw looks for a newer release of this repository when it starts and every 30 minutes, downloads it in the
background, checks its size and SHA-256 checksum (and that it really is a 64-bit Windows program), and swaps it in.
It runs the new version the next time you start it (the **Restart** button in the menu does that at once). The previous
version is kept beside it as `redraw.exe.old`. The line under the title says where it stands, and
**Updates > Auto-update** turns it off.

It talks only to `github.com`, over HTTPS, and sends nothing but a `reDraw/<version>` user agent.

Publishing is automatic: every push to `main` runs [`.github/workflows/release.yml`](.github/workflows/release.yml),
which tests, builds, numbers the build `0.2.<run number>` and publishes a release with `redraw.exe`,
`reDraw-Setup.exe` and `latest.txt` (the version, checksum and address that installed copies read).

## Build

Needs Rust (stable, MSVC toolchain) on Windows.

```
cargo build --release                      # target\release\redraw.exe
cargo test                                 # unit tests

# the installer carries redraw.exe inside it
set REDRAW_EXE=%CD%\target\release\redraw.exe
cargo build --release --manifest-path installer\Cargo.toml
```

`examples/e2e.rs` drives the real exe on the real desktop (glow, lenses, trace, menu, ForceQuit, updater against a
local fake release server) and `examples/lagprobe.rs` measures what each blend costs the desktop compositor. Both
are test tools: they move the mouse and draw on screen, so run them on a machine you are not using.
Build them with `--features "multi-instance local-update"` to run next to an installed copy.

## License

[MIT](LICENSE) (c) 2026 christopher robin fiore.

## Remove

Settings > Apps > reDraw, or run `uninstall.exe` in the install folder. It removes only the files it added and asks
before deleting your settings.
