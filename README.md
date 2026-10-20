# snap-rs

A Rust reimplementation of the concepts behind ESA SNAP: a product/band model, a tiled multi-resolution image viewer with geocoding, pixel info, band-maths expressions and GeoTIFF/COG export. GDAL and PROJ do the heavy lifting for I/O and map projections; the GUI is egui.

<!-- TODO: screenshot of the viewer -->
![screenshot placeholder](docs/screenshot.png)

## Install and run

GDAL and PROJ come from a [pixi](https://pixi.sh) (conda-forge) environment:

```sh
pixi install
pixi run test                       # cargo test --workspace
pixi run run -- path/to/image.tif   # opens the viewer on one or more files
```

Without pixi, install GDAL (3.8-3.9 tested) and PROJ (>= 9.2) with pkg-config, then `cargo run -p snap-gui -- image.tif`. `cargo install` works the same way once those libraries are present.

### WSL2

The Wayland socket lives under WSLg, so set `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir` before launching. File dialogs need `xdg-desktop-portal` or `zenity`; without them, open files on the command line.

## Formats

- GeoTIFF / COG: native reader.
- Any other GDAL raster: opened as a generic product.
- Export: GeoTIFF or COG.

## Docs

- Design spec: [docs/superpowers/specs/2026-10-20-snap-rs-design.md](docs/superpowers/specs/2026-10-20-snap-rs-design.md)
- Plans: [docs/superpowers/plans/](docs/superpowers/plans/)

## Contributing

Test-driven: failing test, minimal code, pass, commit. One crate per concern; dependency direction is `snap-gui -> snap-io-gdal -> snap-io -> snap-core` and `snap-expr -> snap-core`. See [CLAUDE.md](CLAUDE.md) for the full rules. Before a PR, run `pixi run check` (= `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`).
