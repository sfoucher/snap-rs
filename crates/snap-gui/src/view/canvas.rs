use super::gpu::{CanvasCallback, TileDraw};
use super::{level_for_zoom, visible_tiles, ViewSources, ViewState};
use crate::app::AppState;
use eframe::egui_wgpu;
use egui::{pos2, Color32, Key, Modifiers, Pos2, Rect as ERect, Sense, Stroke};
use snap_core::{TileKey, TileState, TILE};

const ZOOM_STEP: f64 = 1.25;
const HATCH_SPACING: f32 = 8.0;

#[allow(dead_code)] // hover_pixel is read by the pixel info task
pub struct CanvasEvent {
    pub hover_pixel: Option<(f64, f64)>,
}

fn fit(view: &mut ViewState, rect: ERect, w: u32, h: u32) {
    view.zoom = (rect.width() as f64 / w as f64).min(rect.height() as f64 / h as f64);
    view.center = (w as f64 / 2.0, h as f64 / 2.0);
}

fn to_image(view: &ViewState, rect: ERect, p: Pos2) -> (f64, f64) {
    let c = rect.center();
    (
        view.center.0 + (p.x - c.x) as f64 / view.zoom,
        view.center.1 + (p.y - c.y) as f64 / view.zoom,
    )
}

fn handle_input(ui: &egui::Ui, resp: &egui::Response, view: &mut ViewState, w: u32, h: u32) {
    let rect = resp.rect;
    if view.zoom <= 0.0 || view.zoom.is_nan() || resp.double_clicked() {
        fit(view, rect, w, h);
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        view.center.0 -= d.x as f64 / view.zoom;
        view.center.1 -= d.y as f64 / view.zoom;
    }
    let Some(pos) = resp.hover_pos() else {
        return;
    };
    let scroll = ui.input(|i| i.raw_scroll_delta.y);
    if scroll != 0.0 {
        let anchor = to_image(view, rect, pos);
        let f = if scroll > 0.0 {
            ZOOM_STEP
        } else {
            1.0 / ZOOM_STEP
        };
        view.zoom = (view.zoom * f).clamp(1e-4, 1e3);
        let c = rect.center();
        view.center = (
            anchor.0 - (pos.x - c.x) as f64 / view.zoom,
            anchor.1 - (pos.y - c.y) as f64 / view.zoom,
        );
    }
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Num0)) {
        view.zoom = 1.0;
    }
}

pub fn canvas_ui(ui: &mut egui::Ui, state: &mut AppState, view_idx: usize) -> CanvasEvent {
    let mut event = CanvasEvent { hover_pixel: None };
    let Some(view) = state.views.get(view_idx) else {
        return event;
    };
    let ViewSources::Single(band_name) = &view.sources else {
        return event;
    };
    let Some(product) = state.product(view.product).cloned() else {
        return event;
    };
    let (w, h, src, no_data) = {
        let p = product.read().unwrap();
        let Some(b) = p.band(band_name) else {
            return event;
        };
        (
            b.width,
            b.height,
            b.source.clone(),
            b.no_data.map(|v| v as f32),
        )
    };
    let band_name = band_name.clone();

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let view = &mut state.views[view_idx];
    handle_input(ui, &resp, view, w, h);
    event.hover_pixel = resp.hover_pos().map(|p| to_image(view, rect, p));

    let max_level = src.levels().max(1) - 1;
    let level = level_for_zoom(view.zoom, max_level);
    let ds = (1u64 << level) as f64;
    let c = rect.center();
    let screen = |x: u32, y: u32| {
        pos2(
            (c.x as f64 + (x as f64 * ds - view.center.0) * view.zoom) as f32,
            (c.y as f64 + (y as f64 * ds - view.center.1) * view.zoom) as f32,
        )
    };

    let mut draws = Vec::new();
    let mut failed = Vec::new();
    for (tx, ty, r) in visible_tiles(view, rect, w, h, level) {
        let screen_rect = ERect::from_min_max(screen(r.x, r.y), screen(r.x + r.w, r.y + r.h));
        let key = TileKey {
            product: view.product,
            band: band_name.clone(),
            level,
            tx,
            ty,
        };
        match state.cache.get(&key, &src) {
            TileState::Ready(tile) => draws.push(TileDraw {
                key,
                tile,
                no_data,
                screen: screen_rect,
                uv: ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            }),
            TileState::Pending if level < max_level => {
                let parent = TileKey {
                    level: level + 1,
                    tx: tx / 2,
                    ty: ty / 2,
                    ..key
                };
                if let Some(tile) = state.cache.peek(&parent) {
                    // This tile's footprint inside the coarser tile, in its pixels.
                    let (rows, cols) = tile.shape();
                    let u = |v: u32, origin: u32, len: usize| {
                        (v as f32 / 2.0 - (origin * TILE) as f32) / len as f32
                    };
                    let uv = ERect::from_min_max(
                        pos2(u(r.x, parent.tx, cols), u(r.y, parent.ty, rows)),
                        pos2(u(r.x + r.w, parent.tx, cols), u(r.y + r.h, parent.ty, rows)),
                    );
                    draws.push(TileDraw {
                        key: parent,
                        tile,
                        no_data,
                        screen: screen_rect,
                        uv,
                    });
                }
            }
            TileState::Pending => {}
            TileState::Failed(_) => failed.push(screen_rect),
        }
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let view = &state.views[view_idx];
    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        CanvasCallback {
            id: view_idx as u64,
            frame: ui.ctx().cumulative_pass_nr(),
            stretch: view.stretch.clone(),
            mode: 0,
            lut: view.palette.to_lut(),
            draws,
        },
    ));
    let stroke = Stroke::new(1.0_f32, Color32::from_rgb(200, 60, 60));
    for r in failed {
        let hatch = painter.with_clip_rect(r.intersect(rect));
        hatch.rect_stroke(r, 0.0, stroke);
        let mut x = r.left() - r.height();
        while x < r.right() {
            hatch.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], stroke);
            x += HATCH_SPACING;
        }
    }
    event
}
