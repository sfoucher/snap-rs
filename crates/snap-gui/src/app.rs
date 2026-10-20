use crate::explorer::{explorer_ui, ExplorerAction};
use crate::view::ViewState;
use egui_dock::{DockArea, DockState, NodeIndex};
use snap_core::{Product, ProductId, TileCache, TileKey};
use snap_io::{IoError, Registry};
use std::path::Path;
use std::sync::mpsc::Receiver;
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
        Self {
            registry,
            cache,
            cache_rx,
            products: Vec::new(),
            selected: None,
            views: Vec::new(),
            active_view: None,
            errors: Vec::new(),
        }
    }

    pub fn open_path(&mut self, path: &Path) -> Result<ProductId, IoError> {
        let product = self.registry.open(path)?;
        let id = product.id;
        self.products.push(Arc::new(RwLock::new(product)));
        Ok(id)
    }

    pub fn close_product(&mut self, id: ProductId) {
        self.products.retain(|p| p.read().unwrap().id != id);
        self.cache.evict_product(id);
        self.views.retain(|v| v.product != id);
        self.active_view = None;
        if self.selected.as_ref().is_some_and(|s| s.product() == id) {
            self.selected = None;
        }
    }

    #[allow(dead_code)] // used by the view/panel tasks
    pub fn product(&self, id: ProductId) -> Option<&Arc<RwLock<Product>>> {
        self.products.iter().find(|p| p.read().unwrap().id == id)
    }
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
}

struct Viewer<'a> {
    state: &'a mut AppState,
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
                let title = self.state.views.get(*i).map(|v| v.title.as_str());
                ui.label(title.unwrap_or("Image"));
            }
            _ => {}
        }
    }
}

impl SnapApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
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
        }
    }

    fn open(&mut self, path: &Path) {
        if let Err(e) = self.state.open_path(path) {
            self.state.errors.push(format!("{}: {e}", path.display()));
        }
    }

    fn close_product(&mut self, id: ProductId) {
        // Image(i) indexes state.views, so drop closed tabs and shift the rest.
        let mut next = 0;
        let remap: Vec<Option<usize>> = self
            .state
            .views
            .iter()
            .map(|v| {
                (v.product != id).then(|| {
                    next += 1;
                    next - 1
                })
            })
            .collect();
        self.state.close_product(id);
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
            ExplorerAction::OpenView(product, title) => {
                self.state.views.push(ViewState { product, title });
                let idx = self.state.views.len() - 1;
                self.state.active_view = Some(idx);
                self.dock.push_to_focused_leaf(Pane::Image(idx));
            }
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
        if self.state.cache_rx.try_iter().count() > 0 {
            ctx.request_repaint();
        }
        let mut actions = self.menu(ctx);
        let mut viewer = Viewer {
            state: &mut self.state,
            actions: Vec::new(),
        };
        DockArea::new(&mut self.dock).show(ctx, &mut viewer);
        actions.append(&mut viewer.actions);
        for a in actions {
            self.handle(a);
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
mod fixtures;

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
    fn open_bad_path_returns_error_not_panic() {
        assert!(AppState::new().open_path(Path::new("nope.tif")).is_err());
    }
}
