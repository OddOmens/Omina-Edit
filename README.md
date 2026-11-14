# Canvas

Photo editing, painting and vector design in one app, built in Rust beside Studio: Photoshop-style
layers, selections, brushes, retouching, adjustments and filters, Lightroom-style Develop settings, and
Illustrator-style shapes, paths and Pathfinder. It uses Studio's stack (GPUI + gpui-kit) and Studio's
look: the same themes and accents, top bar of tools, rail, movable dock panels, folding cards, value
fields, sliders, dialogs, start screen and settings.

## Layout

```
crates/canvas-core    The document model and everything that edits it, with no UI: projects holding
                      documents and media; pixel, shape, text, fill, adjustment and group layers; masks,
                      clipping, layer styles, blend modes; selections; the brush engine, fills and
                      gradients; filters; adjustment layers; Develop (Lightroom) settings; histograms and
                      auto corrections; Pathfinder booleans and outline stroke (i_overlay); picture files,
                      SVG in and out, and the .canvas project file (a zip of JSON plus PNGs, checked on load).
crates/canvas-render  The compositor (tiny-skia, on the CPU, threaded with rayon): draws a document or any
                      part of it at any scale. Every Photoshop blend mode, masks, clipping masks,
                      adjustment layers, isolated and pass-through groups, layer styles (drop and inner
                      shadow, outer glow, stroke, color overlay), text with system fonts (cosmic-text),
                      vector shapes kept sharp at any zoom, halved copies of big pictures when zoomed out,
                      and cached Develop results.
apps/canvas           Desktop app (GPUI + gpui-kit). UI only: panels read `EditorState` and forward input.
```

Inside `apps/canvas/src`:

- `state/`: every editing action, unit-tested without a window. `mod.rs` holds documents, layers, masks
  and colors; `tools.rs` what each tool does on press, drag and release (move, Free Transform, marquees,
  lassos, magic wand, crop, eyedropper, brush, pencil, eraser, spot healing, healing, clone stamp, blur,
  sharpen, smudge, dodge, burn, sponge, gradient, paint bucket, pen, direct selection, type, shapes);
  `edit.rs` the menu commands (canvas and image size, rotate, trim, crop, auto tone, clipboard, fill and
  stroke, merge, flatten, rasterize, Pathfinder, align, filters, Develop); `history.rs` undo and the
  History panel's named steps (whole-project snapshots that share unchanged pixels).
- `workspace.rs`: the window, menu bar, shortcuts and dock; `commands.rs` what each command does.
- `panels/`: one module per panel. `viewer.rs` is the canvas: it draws only what is on screen at screen
  resolution, redraws just the brushed area while painting, and draws the tools' marks (marching ants,
  transform and crop boxes, path points, brush outline, typing caret). `options.rs` is the tool options
  bar; `layers.rs`, `properties.rs` (Properties, Develop and Styles tabs), `history.rs`, `align.rs`
  (with Pathfinder); the side panels the rail picks, `media.rs`, `libraries.rs`, `effects.rs`,
  `elements.rs`, `mockups.rs`, `templates.rs`, `brushes.rs` and `assistant.rs` (the Design Assistant);
  dialogs in `dialogs.rs`, `filter_dialog.rs` (live preview), `export.rs`, `settings.rs`, `start_screen.rs`,
  `color_picker.rs`; and `onboarding.rs`, the welcome setup shown on first launch (as Studio's). `chrome.rs`,
  `dock.rs` and `theme.rs` are Studio's, so both apps look and behave alike.
- `features.rs`: the tools and features Settings ▸ Tools & Features can turn off, as Studio's; one that is
  off is hidden from the top bar, rail, menus, panels, the assistant's answers and the keymap.
- `keymap.rs`: Photoshop's and Illustrator's keys (V, M, L, W, C, I, B, E, J, S, G, O, P, T, U, H, Z, X, D,
  [ ], ⌘T, ⌘J, ⌘E, ⌘G, ⌘L, ⌘M, ⌘U…). Tooltips read keys from here.
- `ui_tests.rs`: headless UI tests, the real window driven with simulated clicks and keys. Tests keep
  their settings in a temporary folder of their own, never the real settings file.

## Commands

```bash
cargo run -p canvas --release            # run the editor
cargo run -p canvas --release -- a.jpg   # open a picture, SVG or .canvas file straight away
cargo test --workspace                   # run all tests
cargo clippy --workspace --all-targets
```

## Conventions

- Document changes go through `EditorState::doc_mut` (or, for painting, the brushed area only), which
  bumps `revision` and marks what the canvas must redraw. Undo needs nothing from new edits: the
  workspace records a step whenever the project changed and no drag is in progress, named by
  `state.label`.
- Pixels are premultiplied RGBA8 (`canvas_core::Bitmap`), shared with `Arc`; change a pixel layer only
  through `Raster::pixels_mut` or `set_pixels`, which renew its change stamp so caches notice.
- Layers are stored bottom to top; the Layers panel lists them top to bottom.
- Every visible control works or is not shown. Project files are untrusted input (`file::check`).
- `unsafe` is forbidden workspace-wide.

## License

GPL-3.0-or-later, as Studio. See `LICENSE`.
