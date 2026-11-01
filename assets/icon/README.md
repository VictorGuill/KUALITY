# App icon

Logo "Geometric K". Everything here is generated from vectors by `src/make_icons.py`
(`python assets/icon/src/make_icons.py`; needs `resvg` on PATH and Pillow).

| File | Used for |
|---|---|
| `kuality.svg` | Master app tile (256 × 256). |
| `kuality.ico` | Embedded in `kuality.exe` as icon resource 1 (`crates/core/build.rs`): Explorer, taskbar, Start menu, settings window (GPUI loads resource 1), installer (`SetupIconFile`). 16–256 px; 16–32 px are hand-tuned. |
| `tray-white.ico`, `tray-black.ico` | Icon resources 2 and 3: the monochrome tray glyph, white on a dark taskbar, black on a light one. The core picks one from the taskbar's pixels (like the widgets' text color) and switches on a light/dark change. 16–48 px. |
| `tray-white.svg`, `tray-black.svg` | Tray glyph vectors (16 px design). |
| `png/kuality-<n>.png` | App tile PNGs, 16–512 px (README, store pages; not used by the build). |
| `wizard-small-55.bmp`, `wizard-small-110.bmp` | Inno Setup `WizardSmallImageFile` (100 % / 200 %). |
| `wizard-164x314.bmp`, `wizard-328x628.bmp` | Inno Setup `WizardImageFile` (100 % / 200 %). |
| `src/` | Per-size sources and the generator. |

Rule for the `.ico` files: images up to 64 px are stored as BMP, larger ones as PNG. A PNG entry at
tray size makes the core load the Windows imaging codecs (`WindowsCodecs.dll`, about 1 MB more
working set). `make_icons.py` already writes them that way.
