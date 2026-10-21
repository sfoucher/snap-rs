# snap-rs

Rust reimplementation of the concepts behind ESA SNAP: product/band model, tiled image viewer, processing operators.

## Read first
- Spec: `docs/superpowers/specs/2026-10-20-snap-rs-design.md`
- Plans: `docs/superpowers/plans/` — implement task by task with `superpowers:subagent-driven-development`. Each task has its own test cycle and commit.

## Rules
- Dependency direction is strict: `snap-gui → snap-io-gdal → snap-io → snap-core`; `snap-expr → snap-core`. Never add `gdal` or `egui` below their crate.
- Tile size 512, edge tiles clipped. Pixel (0,0) is the corner of the top-left pixel; centre is (0.5, 0.5).
- One `thiserror` enum per crate. GDAL errors go through the error handler, never stderr.
- TDD: failing test → minimal code → pass → commit. No code without a test in the same task.
- No new dependency unless the plan names it. No abstraction "for later". Exception: `embed-resource`, a Windows-only build dependency of `snap-gui` that puts the product name and version in the exe.
- One GDAL `Dataset` per product, shared by all its bands (`SharedDs`). Pixel-interleaved GeoTIFFs decode every band per block, so a dataset per band ran a 158-band file out of memory.
- Band names: GDAL description, else SNAP's DIMAP header (TIFF tag 65000, `dimap_names.rs`), else `band_N`.
- Dock layout (`snap-gui/src/app.rs`): open image views with `push_image_tab` and drop them with `close_image_tabs`. Never `retain_tabs` for image tabs: it collapses emptied leaves and mangles the layout. The tab's X button collapses the centre leaf too, which `push_image_tab` rebuilds.
- Follow the plan's exact names and signatures; if one can't work, say so in the commit message and the task report instead of renaming silently.

## Environment
- Develop in WSL2 or Linux; check the GUI natively on Windows/macOS.
- `pixi install` provides GDAL/PROJ. Run `pixi run test` (= `cargo test --workspace`).
- Before claiming a task done: `cargo fmt --check && cargo clippy --workspace -D warnings && cargo test --workspace` (= `pixi run check`).
- Windows: build from a "Developer PowerShell for VS 2022", or run `vcvars64.bat` first. A plain shell fails with `LNK1181: cannot open input file 'kernel32.lib'`. `pixi run` drops backslashes, so write paths with `/` (`pixi run target/release/snap-gui.exe`).

## Release (Windows)
- License: MIT. The README has the SignPath "Code signing policy" section; keep it true.
- Tag `vX.Y.Z` must equal the `snap-gui` crate version, or the Release workflow fails its first step.
- Pushing the tag runs `.github/workflows/release.yml`: `ci/package-windows.ps1` stages the exe, its DLLs (found from the PE import tables) and `share/{gdal,proj}`, then writes `dist/snap-rs-X.Y.Z-win64.zip` and `dist/snap-rs-X.Y.Z.msi` (`installer/snap-rs.wxs`), and `gh release create` publishes both. The release is public.
- WiX must be 5.0.2: v7 refuses to run until you accept a paid-support EULA.
- The exe and MSI are not signed yet. Signing order once it exists: sign the exe, build the MSI, sign the MSI.

## Commit messages
`type(crate): summary` — e.g. `feat(core): pyramid with sidecar levels`.
