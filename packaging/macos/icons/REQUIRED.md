# App icon requirements (temporary checklist)

Delete this file once the icons are in place and wired into the bundle.

## What the bundle needs

macOS wants a single `zeDB.icns` file inside the app at
`Contents/Resources/zeDB.icns`, referenced from `Info.plist` via:

```xml
<key>CFBundleIconFile</key>
<string>zeDB</string>
```

The `.icns` is generated from the PNG set below with:

```sh
iconutil -c icns packaging/macos/icons/zeDB.iconset -o packaging/macos/icons/zeDB.icns
```

## Required PNG files

Drop these into `packaging/macos/icons/zeDB.iconset/`. The filenames are
mandated by `iconutil`; do not rename them.

| Filename | Pixel dimensions | Represents |
|---|---|---|
| `icon_16x16.png` | 16 x 16 | 16pt @1x (Finder list views, menu bar) |
| `icon_16x16@2x.png` | 32 x 32 | 16pt @2x |
| `icon_32x32.png` | 32 x 32 | 32pt @1x |
| `icon_32x32@2x.png` | 64 x 64 | 32pt @2x |
| `icon_128x128.png` | 128 x 128 | 128pt @1x |
| `icon_128x128@2x.png` | 256 x 256 | 128pt @2x |
| `icon_256x256.png` | 256 x 256 | 256pt @1x |
| `icon_256x256@2x.png` | 512 x 512 | 256pt @2x |
| `icon_512x512.png` | 512 x 512 | 512pt @1x |
| `icon_512x512@2x.png` | 1024 x 1024 | 512pt @2x (Dock, App Store, Quick Look) |

## Format rules

- PNG, RGBA (8 bits per channel), with a real alpha channel.
- sRGB color profile (or no embedded profile; avoid Display P3 exports).
- No interlacing.
- Square canvas exactly matching the dimensions above; no padding baked in
  beyond the design's own margins.
- 72 DPI metadata is conventional but the pixel dimensions are what matter.

## Design rules (Big Sur and later style)

- Rounded-rectangle "squircle" shape filling most of the canvas, with
  transparent corners; macOS does NOT mask the icon for you, the shape must
  be drawn into the artwork.
- Standard grid: on the 1024 px master the squircle spans roughly 824 x 824 px
  centered, leaving about 100 px of transparent margin on each side. Scaled
  proportionally for the smaller sizes.
- Subtle drop shadow may be baked in (Apple's template does this).
- Design the 1024 px master first, then export/downscale; hand-tweak the
  16/32 px sizes if fine detail turns to mush.

## Workflow

1. Produce the 1024 x 1024 master (`icon_512x512@2x.png`).
2. Export the other nine sizes into `zeDB.iconset/`.
3. Run the `iconutil` command above; commit the generated `zeDB.icns`.
4. Add `CFBundleIconFile` to `packaging/macos/Info.plist` and copy
   `zeDB.icns` into `Contents/Resources/` in the bundle script.
5. Delete this file.
