# snap-rs — Design Spec

Date: 2026-10-20
Status: approved in conversation, pending written review

## 1. Purpose

snap-rs is an open-source Rust reimplementation of the concepts behind ESA SNAP (Sentinel Application Platform): a product/band data model, a tiled image viewer, and a Graph Processing Framework (GPF) for processing operators.

Audience: the wider remote-sensing community. Cross-platform desktop (Linux, macOS, Windows).

"Clone" means the same concepts and workflow, not a port of SNAP's Java classes or pixel-for-pixel UI parity.

## 2. Scope

### v1 — Image viewer

Must:
- Open GeoTIFF and Cloud-Optimized GeoTIFF natively, with correct geocoding and a metadata tree.
- Open any other GDAL-readable raster as a generic product (geocoding and metadata as GDAL exposes them, no format-specific handling).
- Provide SNAP's image-view features: contrast stretch, RGB composites, pixel info, masks, band maths.
- Export a subset to GeoTIFF.
- Ship the `Operator` trait with two built-in operators (`BandMaths`, `Subset`) and a headless CLI runner, so the plugin layer has a proven seat.

Out of scope for v1:
- Sentinel SAFE readers (S1, S2, S3), BEAM-DIMAP read/write, NetCDF-specific handling.
- Tie-point geocoding.
- Vector overlays, pins, world map, synchronised views, layer manager.
- WASM plugin host.
- Bundled installers (v1 documents a conda/pixi environment).

### v2 — Plugins (direction only)

WASM modules implementing the `Operator` contract, hosted via `wasmtime` or `extism`. Tiles cross the boundary through linear memory; parameters are JSON.

## 3. Decisions

| Decision | Choice | Reason |
|---|---|---|
| Native dependencies | Allowed: GDAL, PROJ, and what they pull in | Fastest route to broad format support; JP2 in pure Rust is not realistic |
| GUI framework | egui / eframe with wgpu | Fast iteration, GPU textures for tiles |
| Arrays | ndarray | Zero-copy tile views |
| Projections | `proj` crate | Geocoding and CRS transforms |
| Expression parser | Hand-rolled with `chumsky` (or `pest`) | SNAP expression syntax is small |
| Plugin model (v2) | WASM | Sandboxed, stable ABI, multi-language |
| Parallelism | `rayon` | Tile work is CPU-bound and independent |
| Tile size | 512 × 512, fixed | Matches COG conventions; simple cache keys |

## 4. Architecture

### 4.1 Workspace

```
snap-rs/
  crates/
    snap-core/      Product, Band, Mask, MetadataElement, GeoCoding,
                    RasterSource trait, Tile, Pyramid, TileCache
    snap-expr/      Band-maths expression parser + evaluator (SNAP syntax)
    snap-io/        ProductReader/Writer traits, format registry, probing
    snap-io-gdal/   GDAL-backed reader/writer; dedicated GeoTIFF/COG path,
                    generic path for other formats
    snap-gpf/       Operator trait, OperatorSource, graph DAG,
                    built-ins BandMaths and Subset
    snap-gui/       egui/eframe + wgpu desktop app
    snap-cli/       gpt-like headless runner (also the CI harness)
```

Dependency direction is strictly downward: `gui → gpf → io → core`. `expr` and `io-gdal` attach at the side. Two invariants:

- Nothing below `snap-gui` knows a GUI exists.
- Nothing below `snap-io-gdal` knows GDAL exists.

`snap-plugin` (WASM host) does not exist in v1. Its seat is a second `Operator` implementation in `snap-gpf`.

### 4.2 Core data model (`snap-core`)

```rust
pub struct Product {
    pub name: String,
    pub product_type: String,          // "GeoTIFF", "GDAL", later "S2_MSI_L2A"
    pub width: u32,
    pub height: u32,
    pub bands: Vec<Band>,
    pub masks: Vec<Mask>,
    pub metadata: MetadataElement,     // tree of attributes and children
    pub geocoding: Arc<dyn GeoCoding>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

pub struct Band {
    pub name: String,
    pub data_type: DataType,           // U8, I8, U16, I16, U32, I32, F32, F64
    pub width: u32,
    pub height: u32,                   // may differ from product size
    pub no_data: Option<f64>,
    pub scaling: Scaling,              // factor, offset, log10 flag
    pub unit: Option<String>,
    pub valid_pixel_expr: Option<String>,
    pub source: Arc<dyn RasterSource>,
    pub stats: OnceLock<BandStats>,    // histogram, min, max; lazy
}

pub struct Mask {
    pub name: String,
    pub expr: String,                  // evaluates to bool
    pub color: [u8; 4],
    pub opacity: f32,
    pub source: Arc<dyn RasterSource>, // ExprSource producing U8 0/1
}

pub struct MetadataElement {
    pub name: String,
    pub attributes: Vec<MetadataAttribute>,   // name, value (string/number/array), unit, description
    pub children: Vec<MetadataElement>,
}
```

### 4.3 RasterSource — the central abstraction

```rust
pub trait RasterSource: Send + Sync {
    fn data_type(&self) -> DataType;
    fn size(&self) -> (u32, u32);
    fn levels(&self) -> u8;
    fn read_tile(&self, level: u8, tx: u32, ty: u32) -> Result<Tile, IoError>;
}

pub enum Tile {
    U8(Array2<u8>), I8(Array2<i8>), U16(Array2<u16>), I16(Array2<i16>),
    U32(Array2<u32>), I32(Array2<i32>), F32(Array2<f32>), F64(Array2<f64>),
}
```

Tiles are 512 × 512 except at the right and bottom edges, where they are clipped. Level 0 is full resolution; level n is downsampled by 2ⁿ.

v1 implementations:

- `GdalSource` (`snap-io-gdal`): reads via GDAL. Uses the file's own overviews when present; otherwise `Pyramid` builds them.
- `ExprSource` (`snap-expr`): evaluates a band-maths expression. Reads referenced bands' tiles at the same level and evaluates per pixel. Output type is F32 unless the expression is boolean (U8).
- `OperatorSource` (`snap-gpf`): calls `Operator::compute_tile`.

### 4.4 Pyramid and TileCache

`Pyramid` wraps a `RasterSource` with fewer levels than needed and produces missing levels by 2× downsampling (mean for numeric, nearest for integer flag bands). Levels are persisted as a sidecar in a cache directory keyed by (path, mtime, size) so they survive restarts.

`TileCache` is a global, memory-bounded LRU keyed by `(product_id, band_name, level, tx, ty)`. All consumers (viewer, operators, CLI) request tiles through it. On a miss it schedules the read on a `rayon` pool and returns `Pending`; completion is signalled over a channel so the GUI can repaint. The viewer never calls `read_tile` directly.

### 4.5 GeoCoding

```rust
pub trait GeoCoding: Send + Sync {
    fn pixel_to_geo(&self, x: f64, y: f64) -> Option<GeoPos>;   // lon/lat WGS84
    fn geo_to_pixel(&self, pos: GeoPos) -> Option<PixelPos>;
    fn crs(&self) -> Option<&Crs>;
    fn pixel_to_crs(&self, x: f64, y: f64) -> Option<(f64, f64)>;
}
```

v1 implements `CrsGeoCoding` (affine geotransform + CRS via `proj`) and `NoGeoCoding`. `TiePointGeoCoding` is reserved for S1/S3 work.

### 4.6 Expressions (`snap-expr`)

SNAP-compatible syntax: arithmetic (`+ - * / %`, `^`), comparison, logical (`&& || !`), ternary `? :`, functions (`sqrt`, `exp`, `log`, `log10`, `abs`, `min`, `max`, `pow`, `sin`, `cos`, `tan`, `nan()`, `inf()`), constants (`PI`, `E`, `NaN`), identifiers resolving to band names, and `X`/`Y`/`LAT`/`LON` pseudo-bands.

Parsed once to an AST with source spans. Compiled to a closure over typed tiles. Parse errors carry spans so the UI can underline them.

### 4.7 IO (`snap-io`, `snap-io-gdal`)

```rust
pub trait ProductReader: Send + Sync {
    fn can_read(&self, path: &Path) -> Probe;        // No, Maybe(priority), Yes(priority)
    fn read(&self, path: &Path) -> Result<Product, IoError>;
}
pub trait ProductWriter: Send + Sync {
    fn write(&self, product: &Product, path: &Path, opts: &WriteOpts) -> Result<(), IoError>;
}
```

A `Registry` holds readers and picks the highest-priority `Yes`. `snap-io-gdal` registers two readers: `GeoTiffReader` (high priority on `.tif/.tiff`, fills a proper `MetadataElement` tree from TIFF/GeoTIFF tags and builds `CrsGeoCoding`) and `GdalGenericReader` (low priority, anything GDAL opens). One writer: `GeoTiffWriter` (tiled, with overviews, optional COG layout).

### 4.8 GPF (`snap-gpf`)

```rust
pub trait Operator: Send + Sync {
    fn descriptor(&self) -> OperatorDescriptor;   // name, version, JSON Schema for params, input arity
    fn initialize(&mut self, sources: &[Arc<Product>], params: &Params) -> Result<Product, GpfError>;
    fn compute_tile(&self, band: &str, level: u8, tx: u32, ty: u32) -> Result<Tile, GpfError>;
}
```

`Params` is a `serde_json::Value` validated against the descriptor's schema. The product returned by `initialize` has bands backed by `OperatorSource`, so output is lazy and tiled.

Boundary rules enforced from v1 so WASM can implement the trait later:
- Operators receive and return only `Product` metadata, `Tile`, and `Params`. No `Arc<dyn RasterSource>` crosses the operator boundary; operators request input tiles through a `TileProvider` handle owned by the host.
- Parameters are JSON.

Built-ins: `BandMaths` (wraps `ExprSource`), `Subset` (region and band selection). A `Graph` is a DAG of operator nodes loaded from TOML; `snap-cli run graph.toml` executes it and writes outputs.

### 4.9 Viewer (`snap-gui`)

Framework: egui/eframe, `egui_dock` for docking, wgpu for the image canvas.

Layout: Product Explorer (left), image views as centre tabs, tool windows (Colour Manipulation, Pixel Info, Mask Manager, Metadata, Band Maths) docked right/bottom. One `AppState`; panels are functions of it.

Image canvas: per view, `ViewState { sources, zoom, centre, stretch, palette }`. Each frame: compute pyramid level from zoom → compute visible tile coordinates → request from `TileCache` → draw. Cached tiles are kept as wgpu textures; missing tiles draw from the next coarser level until they arrive. One fragment shader applies stretch (min/max, gamma), palette lookup, no-data transparency; RGB composites use the same shader with three sources. Masks are additional textures composited with colour and opacity. Changing a slider never re-reads tiles.

Colour manipulation: histogram from `BandStats` (estimated from the coarsest level, refined in background); min/max, percentile stretch (default 2–98 %), gamma; palettes loadable from SNAP `.cpd` files.

Pixel Info: image x/y, lat/lon, CRS coordinates, every band's value at cursor (from cached tiles).

Band Maths dialog: expression field with live parse errors; creates a virtual `Band` (`ExprSource`) or, with "create as mask", a `Mask`.

Product Explorer: tree product → Bands / Masks / Metadata. Double-click opens a view; context menu: Properties, Band Maths, Remove, Export subset.

Export: current product or subset to GeoTIFF via `GeoTiffWriter`.

### 4.10 CLI (`snap-cli`)

`snap-cli info <file>` prints the product tree. `snap-cli run <graph.toml>` executes a GPF graph headlessly. It depends on `snap-gpf`, never on `snap-gui`, which keeps the core GUI-independent and gives CI a full-pipeline test.

## 5. Error handling

One `thiserror` enum per crate: `IoError`, `ExprError`, `GpfError`, `GeoError`. `snap-gui` converts them to user-facing messages at the boundary.

- Tile read failures are per tile: the canvas draws a hatched placeholder and logs; the view stays open.
- GDAL errors are captured through its error handler and wrapped, never printed to stderr.
- Expression errors are reported at parse time with spans.
- Cache sidecar corruption is treated as a miss: delete and rebuild.

## 6. Testing

- `snap-core`, `snap-expr`: unit tests. Evaluator checked against a table of SNAP-produced results for identical inputs.
- `snap-io-gdal`: fixture GeoTIFFs generated by a test helper (small, tiled, with and without overviews, several CRSs and data types, with and without no-data). Golden tests on geocoding round-trips against `gdaltransform` output.
- `snap-gpf`: `BandMaths` compared pixel-for-pixel with `ExprSource`; `Subset` against `gdal_translate -srcwin`.
- `snap-cli`: CI runs a graph on fixtures and diffs the output GeoTIFF against a checked-in reference.
- `snap-gui`: builds and renders one frame off-screen; visual behaviour verified by a manual checklist per release.
- CI matrix: Linux, macOS, Windows; GDAL from system packages or vcpkg.

## 7. Distribution (v1)

Documented `pixi`/`conda` environment providing GDAL and PROJ, plus `cargo install`. Bundled installers are a v2 concern.

## 8. Risks

- JPEG2000 and SAFE support lean entirely on GDAL; accept this.
- Geocoding correctness is where clones fail; golden tests against GDAL from day one.
- Scope pull toward SAR; v1 is optical/generic raster only.
- egui lists with thousands of metadata rows need virtualisation.
