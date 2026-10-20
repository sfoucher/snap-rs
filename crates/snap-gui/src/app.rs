use crate::dialogs::rgb::rgb_dialog;
use crate::explorer::{explorer_ui, ExplorerAction};
use crate::panels::color::{color_panel_ui, default_stretch};
use crate::panels::pixel_info::{pixel_info_at, pixel_info_ui};
use crate::view::{canvas_ui, GpuState, Stretch, ViewSources, ViewState};
use egui_dock::{DockArea, DockState, NodeIndex};
use snap_core::{
    compute_stats_coarsest, BandStats, DataType, Product, ProductId, TileCache, TileKey,
};
use snap_io::{IoError, Registry};
use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const MIN_CACHE: u64 = 512 << 20;
const TOAST_SECS: u64 = 5;

pub struct AppState {
    pub registry: Registry,
    pub cache: Arc<TileCache>,
    pub cache_rx: Receiver<TileKey>,
    pub products: Vec<Arc<RwLock<Product>>>,
    pub selected: Option<NodeRef>,
    pub views: Vec<ViewState>,
    pub active_view: Option<usize>,
    pub errors: Vec<String>,
    /// Bands whose background statistics job has been started (never retried).
    stats_started: HashSet<(ProductId, String)>,
    stats_failed: HashSet<(ProductId, String)>,
    stats_tx: Sender<StatsDone>,
    stats_rx: Receiver<StatsDone>,
}

type StatsDone = (ProductId, String, Result<(), String>);

/// Sets the default stretch on each channel of `view` showing `band` that is still on
/// the data-type range. Called only when statistics arrive or a view opens.
fn apply_default_stretch(view: &mut ViewState, band: &str, dt: DataType, stats: &BandStats) {
    if stats.sample_count == 0 {
        return;
    }
    let names = match &view.sources {
        ViewSources::Single(n) => vec![n.clone()],
        ViewSources::Rgb(n) => n.to_vec(),
    };
    for (ch, n) in names.iter().enumerate() {
        if n == band && view.stretch[ch] == Stretch::for_type(dt) {
            view.stretch[ch] = default_stretch(stats);
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum NodeRef {
    Product(ProductId),
    Band(ProductId, String),
    Mask(ProductId, String),
    Metadata(ProductId),
}

impl NodeRef {
    #[allow(dead_code)] // used by the view/panel tasks
    pub fn product(&self) -> ProductId {
        match self {
            NodeRef::Product(p)
            | NodeRef::Band(p, _)
            | NodeRef::Mask(p, _)
            | NodeRef::Metadata(p) => *p,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        let mut registry = Registry::new();
        snap_io_gdal::register(&mut registry);
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let budget = (sys.total_memory() / 4).max(MIN_CACHE) as usize;
        let (cache, cache_rx) = TileCache::new(budget);
        let (stats_tx, stats_rx) = channel();
        Self {
            registry,
            cache,
            cache_rx,
            products: Vec::new(),
            selected: None,
            views: Vec::new(),
            active_view: None,
            errors: Vec::new(),
            stats_started: HashSet::new(),
            stats_failed: HashSet::new(),
            stats_tx,
            stats_rx,
        }
    }

    /// Starts missing band statistics on background threads (no product lock is held
    /// while computing) and handles finished ones: the default stretch is applied once,
    /// when a band's statistics arrive, and failures are recorded (never retried).
    pub fn drive_stats(&mut self, ctx: &egui::Context) {
        for (pid, name, res) in self.stats_rx.try_iter().collect::<Vec<_>>() {
            match res {
                Ok(()) => self.apply_default(pid, &name),
                Err(e) => {
                    self.errors.push(format!("statistics for {name}: {e}"));
                    self.stats_failed.insert((pid, name));
                }
            }
        }
        let mut wanted = Vec::new();
        for v in &self.views {
            let names = match &v.sources {
                ViewSources::Single(n) => vec![n.clone()],
                ViewSources::Rgb(n) => n.to_vec(),
            };
            wanted.extend(names.into_iter().map(|n| (v.product, n)));
        }
        for (pid, name) in wanted {
            if self.stats_started.contains(&(pid, name.clone())) {
                continue;
            }
            let Some(prod) = self.product(pid).cloned() else {
                continue;
            };
            let job = prod
                .read()
                .unwrap()
                .band(&name)
                .filter(|b| b.stats.get().is_none())
                .map(|b| (b.source.clone(), b.no_data));
            let Some((src, nd)) = job else { continue };
            self.stats_started.insert((pid, name.clone()));
            let (ctx, tx) = (ctx.clone(), self.stats_tx.clone());
            std::thread::spawn(move || {
                let res = compute_stats_coarsest(&src, nd)
                    .map_err(|e| e.to_string())
                    .map(|st| {
                        if let Some(b) = prod.read().unwrap().band(&name) {
                            let _ = b.stats.set(st);
                        }
                    });
                let _ = tx.send((pid, name, res));
                ctx.request_repaint();
            });
        }
    }

    fn band_stats(&self, pid: ProductId, band: &str) -> Option<(DataType, BandStats)> {
        let p = self.product(pid)?.read().unwrap();
        let b = p.band(band)?;
        b.stats.get().cloned().map(|st| (b.data_type, st))
    }

    /// Applies the default stretch to every view of `band` (its statistics just arrived).
    pub fn apply_default(&mut self, pid: ProductId, band: &str) {
        if let Some((dt, st)) = self.band_stats(pid, band) {
            for v in self.views.iter_mut().filter(|v| v.product == pid) {
                apply_default_stretch(v, band, dt, &st);
            }
        }
    }

    /// Pushes a single-band view and returns its index; the default stretch is applied
    /// to this view only, so other views' user stretches are left alone.
    pub fn open_view(&mut self, pid: ProductId, band: &str) -> Option<usize> {
        let mut view = {
            let p = self.product(pid)?.read().unwrap();
            let b = p.band(band)?;
            let title = format!("{}: {}", p.name, b.name);
            ViewState::single(pid, band, b.data_type, (b.width, b.height), title)
        };
        if let Some((dt, st)) = self.band_stats(pid, band) {
            apply_default_stretch(&mut view, band, dt, &st);
        }
        self.views.push(view);
        Some(self.views.len() - 1)
    }

    /// Pushes an RGB view of `bands` (r, g, b) and returns its index; like `open_view`,
    /// the default stretch is applied to this view only.
    pub fn open_rgb_view(&mut self, pid: ProductId, bands: [String; 3]) -> Option<usize> {
        let mut view = {
            let p = self.product(pid)?.read().unwrap();
            let [r, g, b] = [&bands[0], &bands[1], &bands[2]].map(|n| p.band(n));
            let (r, g, b) = (r?, g?, b?);
            let title = format!("{}: RGB({},{},{})", p.name, bands[0], bands[1], bands[2]);
            ViewState {
                sources: ViewSources::Rgb(bands.clone()),
                stretch: [r, g, b].map(|b| Stretch::for_type(b.data_type)),
                ..ViewState::single(pid, &bands[0], r.data_type, (r.width, r.height), title)
            }
        };
        for name in &bands {
            if let Some((dt, st)) = self.band_stats(pid, name) {
                apply_default_stretch(&mut view, name, dt, &st);
            }
        }
        self.views.push(view);
        Some(self.views.len() - 1)
    }

    pub fn stats_failed(&self, pid: ProductId, band: &str) -> bool {
        self.stats_failed.contains(&(pid, band.to_string()))
    }

    pub fn open_path(&mut self, path: &Path) -> Result<ProductId, IoError> {
        let product = self.registry.open(path)?;
        let id = product.id;
        self.products.push(Arc::new(RwLock::new(product)));
        Ok(id)
    }

    /// Opens `path`, queuing a toast instead of returning the error.
    pub fn open_or_report(&mut self, path: &Path) {
        if let Err(e) = self.open_path(path) {
            self.errors.push(format!("{}: {e}", path.display()));
        }
    }

    pub fn close_product(&mut self, id: ProductId) {
        self.products.retain(|p| p.read().unwrap().id != id);
        self.cache.evict_product(id);
        let remap = view_remap(&self.views, id);
        self.active_view = self
            .active_view
            .and_then(|i| remap.get(i).copied().flatten());
        self.views.retain(|v| v.product != id);
        self.stats_started.retain(|(p, _)| *p != id);
        self.stats_failed.retain(|(p, _)| *p != id);
        if self.selected.as_ref().is_some_and(|s| s.product() == id) {
            self.selected = None;
        }
    }

    #[allow(dead_code)] // used by the view/panel tasks
    pub fn product(&self, id: ProductId) -> Option<&Arc<RwLock<Product>>> {
        self.products.iter().find(|p| p.read().unwrap().id == id)
    }
}

/// New index of each view once `id`'s views are dropped (None = dropped).
fn view_remap(views: &[ViewState], id: ProductId) -> Vec<Option<usize>> {
    let mut next = 0;
    views
        .iter()
        .map(|v| {
            (v.product != id).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
pub enum Pane {
    Explorer,
    Image(usize),
    Color,
    PixelInfo,
    Metadata,
    Masks,
}

const TOGGLES: [(Pane, &str); 5] = [
    (Pane::Explorer, "Explorer"),
    (Pane::Color, "Color Manipulation"),
    (Pane::PixelInfo, "Pixel Info"),
    (Pane::Metadata, "Metadata"),
    (Pane::Masks, "Masks"),
];

pub struct SnapApp {
    state: AppState,
    dock: DockState<Pane>,
    toasts: Vec<(String, Instant)>,
    show_about: bool,
    rgb_dialog: Option<ProductId>,
    render_state: Option<eframe::egui_wgpu::RenderState>,
    /// Latest canvas hover (view index, level-0 pixel); one frame behind the panel.
    hover: Option<(usize, (f64, f64))>,
}

struct Viewer<'a> {
    state: &'a mut AppState,
    hover: Option<(usize, (f64, f64))>,
    next_hover: Option<(usize, (f64, f64))>,
    actions: Vec<ExplorerAction>,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Pane;

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        match tab {
            Pane::Explorer => "Explorer".into(),
            Pane::Image(i) => self
                .state
                .views
                .get(*i)
                .map_or("Image", |v| &v.title)
                .into(),
            Pane::Color => "Color Manipulation".into(),
            Pane::PixelInfo => "Pixel Info".into(),
            Pane::Metadata => "Metadata".into(),
            Pane::Masks => "Masks".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        match tab {
            Pane::Explorer => self.actions.extend(explorer_ui(ui, self.state)),
            Pane::Image(i) => {
                let ev = canvas_ui(ui, self.state, *i);
                if let Some(h) = ev.hover_pixel {
                    self.next_hover = Some((*i, h));
                }
            }
            Pane::PixelInfo => {
                let info = self.hover.and_then(|(v, (x, y))| {
                    let pid = self.state.views.get(v)?.product;
                    pixel_info_at(self.state, pid, x, y)
                });
                pixel_info_ui(ui, info.as_ref());
            }
            Pane::Color => color_panel_ui(ui, self.state),
            _ => {}
        }
    }

    fn scroll_bars(&self, tab: &Pane) -> [bool; 2] {
        // The canvas fills its tab and owns the wheel for zooming.
        let scroll = !matches!(tab, Pane::Image(_));
        [scroll, scroll]
    }
}

impl SnapApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let render_state = cc.wgpu_render_state.clone();
        if let Some(rs) = &render_state {
            let gpu = GpuState::new(rs);
            rs.renderer.write().callback_resources.insert(gpu);
        }
        let mut dock = DockState::new(Vec::new());
        let tree = dock.main_surface_mut();
        let [centre, _] = tree.split_left(NodeIndex::root(), 0.2, vec![Pane::Explorer]);
        tree.split_right(
            centre,
            0.75,
            vec![Pane::Color, Pane::PixelInfo, Pane::Metadata, Pane::Masks],
        );
        Self {
            state: AppState::new(),
            dock,
            toasts: Vec::new(),
            show_about: false,
            rgb_dialog: None,
            render_state,
            hover: None,
        }
    }

    pub fn open(&mut self, path: &Path) {
        self.state.open_or_report(path);
    }

    fn close_product(&mut self, id: ProductId) {
        // Image(i) indexes state.views, so drop closed tabs and shift the rest.
        let remap = view_remap(&self.state.views, id);
        self.state.close_product(id);
        if let Some(rs) = &self.render_state {
            if let Some(gpu) = rs.renderer.write().callback_resources.get_mut::<GpuState>() {
                gpu.evict_product(id);
            }
        }
        self.dock.retain_tabs(|t| {
            if let Pane::Image(i) = t {
                match remap.get(*i).copied().flatten() {
                    Some(n) => *i = n,
                    None => return false,
                }
            }
            true
        });
    }

    fn handle(&mut self, action: ExplorerAction) {
        match action {
            ExplorerAction::OpenView(product, band) => {
                let Some(idx) = self.state.open_view(product, &band) else {
                    return;
                };
                self.state.active_view = Some(idx);
                self.dock.push_to_focused_leaf(Pane::Image(idx));
            }
            ExplorerAction::OpenRgb(product) => self.rgb_dialog = Some(product),
            ExplorerAction::Remove(NodeRef::Product(id)) => self.close_product(id),
            _ => {}
        }
    }

    fn menu(&mut self, ctx: &egui::Context) -> Vec<ExplorerAction> {
        let mut actions = Vec::new();
        let target = self
            .state
            .active_view
            .and_then(|i| self.state.views.get(i))
            .map(|v| v.product)
            .or(self.state.selected.as_ref().map(NodeRef::product));
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open…").clicked() {
                        ui.close_menu();
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter("GeoTIFF", &["tif", "tiff"])
                            .add_filter("All", &["*"])
                            .pick_files()
                        {
                            for p in paths {
                                self.open(&p);
                            }
                        }
                    }
                    if ui
                        .add_enabled(target.is_some(), egui::Button::new("Close"))
                        .clicked()
                    {
                        actions
                            .extend(target.map(|id| ExplorerAction::Remove(NodeRef::Product(id))));
                        ui.close_menu();
                    }
                    let sel = self.state.selected.as_ref().map(NodeRef::product);
                    if ui
                        .add_enabled(sel.is_some(), egui::Button::new("Export Subset…"))
                        .clicked()
                    {
                        actions.extend(sel.map(ExplorerAction::Export));
                        ui.close_menu();
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    for (pane, name) in TOGGLES {
                        let open = self.dock.find_tab(&pane).is_some();
                        if ui.selectable_label(open, name).clicked() {
                            if open {
                                self.dock.retain_tabs(|t| *t != pane);
                            } else {
                                self.dock.push_to_focused_leaf(pane);
                            }
                            ui.close_menu();
                        }
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                });
            });
        });
        actions
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts
            .extend(self.state.errors.drain(..).map(|e| (e, now)));
        self.toasts
            .retain(|(_, t)| now.duration_since(*t) < Duration::from_secs(TOAST_SECS));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -10.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for (msg, _) in &self.toasts {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, msg);
                    });
                }
            });
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

impl eframe::App for SnapApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped.iter().filter_map(|f| f.path.as_ref()) {
            self.open(f);
        }
        self.state.drive_stats(ctx);
        if self.state.cache_rx.try_iter().count() > 0 {
            ctx.request_repaint();
        }
        let mut actions = self.menu(ctx);
        let mut viewer = Viewer {
            state: &mut self.state,
            hover: self.hover,
            next_hover: None,
            actions: Vec::new(),
        };
        DockArea::new(&mut self.dock).show(ctx, &mut viewer);
        actions.append(&mut viewer.actions);
        if viewer.next_hover != self.hover {
            ctx.request_repaint();
        }
        self.hover = viewer.next_hover;
        if let Some((_, Pane::Image(i))) = self.dock.find_active_focused() {
            self.state.active_view = Some(*i);
        }
        if let Some(i) = self.state.active_view {
            if self.dock.find_tab(&Pane::Image(i)).is_none() {
                self.state.active_view = None;
            }
        }
        for a in actions {
            self.handle(a);
        }
        if let Some(pid) = self.rgb_dialog {
            let mut open = self.state.product(pid).is_some();
            if let Some(bands) = rgb_dialog(ctx, &mut self.state, pid, &mut open) {
                if let Some(idx) = self.state.open_rgb_view(pid, bands) {
                    self.state.active_view = Some(idx);
                    self.dock.push_to_focused_leaf(Pane::Image(idx));
                }
            }
            if !open {
                self.rgb_dialog = None;
            }
        }
        if self.show_about {
            egui::Window::new("About")
                .open(&mut self.show_about)
                .show(ctx, |ui| {
                    ui.label(concat!("snap-rs ", env!("CARGO_PKG_VERSION")));
                });
        }
        self.show_toasts(ctx);
    }
}

#[cfg(test)]
#[path = "../../snap-io-gdal/tests/fixtures.rs"]
pub(crate) mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    use snap_core::TileKey;
    use std::path::Path;

    use super::fixtures;

    #[test]
    fn open_path_adds_product_and_close_removes() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-open");
        let fx = fixtures::make_fixture(
            &dir,
            "a.tif",
            64,
            64,
            snap_core::DataType::U16,
            1,
            false,
            None,
            Some(32633),
        );
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        assert_eq!(st.products.len(), 1);
        let (src, name) = {
            let p = st.product(id).unwrap().read().unwrap();
            (p.bands[0].source.clone(), p.bands[0].name.clone())
        };
        let key = TileKey {
            product: id,
            band: name,
            level: 0,
            tx: 0,
            ty: 0,
        };
        st.cache.get_blocking(&key, &src).unwrap();
        assert!(st.cache.used_bytes() > 0);
        st.close_product(id);
        assert_eq!(st.products.len(), 0);
        assert_eq!(st.cache.used_bytes(), 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn view_remap_drops_closed_product_and_shifts_rest() {
        let v = |p| ViewState {
            product: ProductId(p),
            ..crate::view::test_view(1.0, (0.0, 0.0))
        };
        let views = [v(1), v(2), v(1), v(3)];
        assert_eq!(
            view_remap(&views, ProductId(1)),
            vec![None, Some(0), None, Some(1)]
        );
    }

    #[test]
    fn default_stretch_applied_only_to_untouched_channels() {
        let stats = |n| BandStats {
            min: 0.0,
            max: 100.0,
            mean: 50.0,
            histogram: vec![n; 512],
            bin_width: 100.0 / 512.0,
            sample_count: n as u64 * 512,
            level: 0,
        };
        let mut v = ViewState::single(ProductId(1), "b", DataType::U8, (1, 1), "t".into());
        let user = Stretch {
            min: 5.0,
            max: 9.0,
            gamma: 2.0,
        };
        // empty stats: untouched
        apply_default_stretch(&mut v, "b", DataType::U8, &stats(0));
        assert_eq!(v.stretch[0], Stretch::for_type(DataType::U8));
        // other band: untouched
        apply_default_stretch(&mut v, "x", DataType::U8, &stats(4));
        assert_eq!(v.stretch[0], Stretch::for_type(DataType::U8));
        // arrival: applied
        apply_default_stretch(&mut v, "b", DataType::U8, &stats(4));
        assert!((v.stretch[0].max - 98.0).abs() < 1.0, "{:?}", v.stretch[0]);
        // user-changed channel: untouched
        v.stretch[0] = user.clone();
        apply_default_stretch(&mut v, "b", DataType::U8, &stats(4));
        assert_eq!(v.stretch[0], user);
    }

    #[test]
    fn opening_a_view_leaves_other_views_stretch_alone() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-openview");
        let fx = fixtures::make_fixture(&dir, "a.tif", 8, 8, DataType::U8, 1, false, None, None);
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        let band = st.product(id).unwrap().read().unwrap().bands[0]
            .name
            .clone();
        let stats = BandStats {
            min: 0.0,
            max: 100.0,
            mean: 50.0,
            histogram: vec![4; 512],
            bin_width: 100.0 / 512.0,
            sample_count: 2048,
            level: 0,
        };
        st.product(id).unwrap().read().unwrap().bands[0]
            .stats
            .set(stats)
            .unwrap();
        let a = st.open_view(id, &band).unwrap();
        assert_ne!(st.views[a].stretch[0], Stretch::for_type(DataType::U8));
        // user presses "Min/Max": back on the U8 range
        st.views[a].stretch[0] = Stretch::for_type(DataType::U8);
        st.stats_started.insert((id, band.clone()));
        let b = st.open_view(id, &band).unwrap();
        assert_eq!(st.views[a].stretch[0], Stretch::for_type(DataType::U8));
        assert_ne!(st.views[b].stretch[0], Stretch::for_type(DataType::U8));
        st.close_product(id);
        assert!(st.stats_started.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn open_rgb_view_applies_default_to_new_view_only() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-openrgb");
        let fx = fixtures::make_fixture(&dir, "a.tif", 8, 8, DataType::U8, 3, false, None, None);
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        let names: Vec<String> = st
            .product(id)
            .unwrap()
            .read()
            .unwrap()
            .bands
            .iter()
            .map(|b| b.name.clone())
            .collect();
        let stats = BandStats {
            min: 0.0,
            max: 100.0,
            mean: 50.0,
            histogram: vec![4; 512],
            bin_width: 100.0 / 512.0,
            sample_count: 2048,
            level: 0,
        };
        // statistics for bands 0 and 2 only
        for i in [0, 2] {
            st.product(id).unwrap().read().unwrap().bands[i]
                .stats
                .set(stats.clone())
                .unwrap();
        }
        let a = st.open_view(id, &names[0]).unwrap();
        st.views[a].stretch[0] = Stretch::for_type(DataType::U8); // user choice
        let rgb = [names[2].clone(), names[1].clone(), names[0].clone()];
        let b = st.open_rgb_view(id, rgb.clone()).unwrap();
        let u8r = Stretch::for_type(DataType::U8);
        assert_eq!(st.views[a].stretch[0], u8r);
        let v = &st.views[b];
        assert!(matches!(&v.sources, ViewSources::Rgb(n) if *n == rgb));
        assert_ne!(v.stretch[0], u8r);
        assert_eq!(v.stretch[1], u8r); // no statistics yet
        assert_ne!(v.stretch[2], u8r);
        assert_eq!(v.title, format!("a: RGB({},{},{})", rgb[0], rgb[1], rgb[2]));
        st.close_product(id);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn open_bad_path_returns_error_not_panic() {
        assert!(AppState::new().open_path(Path::new("nope.tif")).is_err());
    }

    #[test]
    fn open_or_report_queues_errors_and_keeps_going() {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir("gui-args");
        let fx = fixtures::make_fixture(
            &dir,
            "a.tif",
            8,
            8,
            snap_core::DataType::U8,
            1,
            false,
            None,
            None,
        );
        let mut st = AppState::new();
        st.open_or_report(Path::new("nope.tif"));
        st.open_or_report(&fx.path);
        assert_eq!(st.products.len(), 1);
        assert_eq!(st.errors.len(), 1);
        assert!(st.errors[0].starts_with("nope.tif: "));
        std::fs::remove_dir_all(dir).ok();
    }
}
