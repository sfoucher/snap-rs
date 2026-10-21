# snap-rs

A Rust reimplementation of the concepts behind ESA SNAP: a product/band model, a tiled multi-resolution image viewer with geocoding, pixel info, band-maths expressions and GeoTIFF/COG export. GDAL and PROJ do the heavy lifting for I/O and map projections; the GUI is egui.

_Screenshot to come._

## Install and run

GDAL and PROJ come from a [pixi](https://pixi.sh) (conda-forge) environment:

```sh
pixi install
pixi run test                       # cargo test --workspace
pixi run run                        # starts the viewer
pixi run cargo run -p snap-gui -- path/to/image.tif   # opens one or more files
```

CI covers Linux (apt GDAL) and Windows (pixi); macOS is untested for now.

Without pixi, install GDAL (3.8-3.9 tested) and PROJ (>= 9.2) with pkg-config, then `cargo run -p snap-gui -- image.tif`. `cargo install` works the same way once those libraries are present.

Rust 1.82 or newer is required.

### Portable install

At startup the viewer looks for `<exe dir>/share/gdal` and `<exe dir>/share/proj` and, when they exist, points `GDAL_DATA` / `PROJ_DATA` at them (unless you already set those variables). Ship the executable with those two folders copied from your GDAL/PROJ install to get a self-contained build.

### Pyramid cache

Images without enough internal overviews get pyramid levels computed once and stored as sidecar files in the user cache directory, under `snap-rs/pyramids/` (Linux `~/.cache/snap-rs/pyramids`, macOS `~/Library/Caches/snap-rs/pyramids`, Windows `%LOCALAPPDATA%\snap-rs\pyramids`). They are keyed by file path, size and modification time; the folder can be deleted at any time and is rebuilt on demand.

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

## License

MIT, see [LICENSE](LICENSE).

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

Windows installers are built from the tagged source by the [Release workflow](.github/workflows/release.yml) on GitHub Actions. Every release is approved by hand before it is signed.

**Team roles.** Samuel Foucher ([@sfoucher](https://github.com/sfoucher)) is the sole committer, reviewer and approver. Pull requests from anyone else are reviewed by the maintainer before they are merged.

**Privacy.** This program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.
