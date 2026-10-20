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
- No new dependency unless the plan names it. No abstraction "for later".
- Follow the plan's exact names and signatures; if one can't work, say so in the commit message and the task report instead of renaming silently.

## Environment
- Develop in WSL2 or Linux; check the GUI natively on Windows/macOS.
- `pixi install` provides GDAL/PROJ. Run `pixi run test` (= `cargo test --workspace`).
- Before claiming a task done: `cargo fmt --check && cargo clippy --workspace -D warnings && cargo test --workspace`.

## Commit messages
`type(crate): summary` — e.g. `feat(core): pyramid with sidecar levels`.
