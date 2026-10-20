use super::gpu::{CanvasCallback, TileDraw};
use super::{level_for_zoom, visible_tiles, ViewSources, ViewState};
use crate::app::AppState;
use eframe::egui_wgpu;
use egui::{pos2, Color32, Key, Modifiers, Pos2, Rect as ERect, Sense, Stroke};
use snap_core::{RasterSource, TileKey, TileState, TILE};
use std::sync::Arc;

const ZOOM_STEP: f64 = 1.25;
const HATCH_SPACING: f32 = 8.0;

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
    let (names, mode) = match &view.sources {
        ViewSources::Single(n) => (vec![n.clone()], 0),
        ViewSources::Rgb(n) => (n.to_vec(), 1),
    };
    let Some(product) = state.product(view.product).cloned() else {
        return event;
    };
    // (source, no_data) per channel; w/h and the pyramid depth come from the first band
    // (ponytail: RGB bands are assumed to share one grid).
    let (w, h, chans, masks) = {
        let p = product.read().unwrap();
        // Visible masks as (name, source, colour, opacity); read each frame so colour
        // and opacity edits only change a uniform.
        let masks: Vec<(String, Arc<dyn RasterSource>, [f32; 4])> = p
            .masks
            .iter()
            .filter(|m| view.show_masks && state.mask_visible(view.product, &m.name))
            .map(|m| {
                let [r, g, b, _] = m.color.map(|c| c as f32 / 255.0);
                (m.name.clone(), m.source.clone(), [r, g, b, m.opacity])
            })
            .collect();
        let mut chans = Vec::new();
        for n in &names {
            let Some(b) = p.band(n) else {
                return event;
            };
            chans.push((b.source.clone(), b.no_data.map(|v| v as f32)));
        }
        let b = p.band(&names[0]).unwrap();
        (b.width, b.height, chans, masks)
    };
    let nch = chans.len();

    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    state.canvas_rects.insert(view_idx, rect);
    let view = &mut state.views[view_idx];
    handle_input(ui, &resp, view, w, h);
    event.hover_pixel = resp.hover_pos().map(|p| to_image(view, rect, p));

    let max_level = chans.iter().map(|c| c.0.levels()).min().unwrap().max(1) - 1;
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
        let keys: Vec<TileKey> = names
            .iter()
            .map(|n| TileKey {
                product: view.product,
                band: n.clone(),
                level,
                tx,
                ty,
            })
            .collect();
        let states: Vec<TileState> = keys
            .iter()
            .zip(&chans)
            .map(|(k, (src, _))| state.cache.get(k, src))
            .collect();
        // Single-band draws repeat channel 0 in the unused slots.
        let ch = |c: usize| c.min(nch - 1);
        let no_data = std::array::from_fn(|c| chans[ch(c)].1);
        if states.iter().any(|s| matches!(s, TileState::Failed(_))) {
            failed.push(screen_rect);
        } else if states.iter().all(|s| matches!(s, TileState::Ready(_))) {
            let tiles = std::array::from_fn(|c| match &states[ch(c)] {
                TileState::Ready(t) => t.clone(),
                _ => unreachable!(),
            });
            draws.push(TileDraw {
                keys: std::array::from_fn(|c| keys[ch(c)].clone()),
                tiles,
                no_data,
                screen: screen_rect,
                uv: ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            });
        } else if level < max_level {
            // Coarser tiles cover this one, but only when every channel has its own.
            let parents: Vec<TileKey> = keys
                .iter()
                .map(|k| TileKey {
                    level: level + 1,
                    tx: tx / 2,
                    ty: ty / 2,
                    ..k.clone()
                })
                .collect();
            let tiles: Vec<_> = parents.iter().map_while(|k| state.cache.peek(k)).collect();
            if tiles.len() == nch {
                // This tile's footprint inside the coarser tile, in its pixels.
                let (rows, cols) = tiles[0].shape();
                let (ptx, pty) = (parents[0].tx, parents[0].ty);
                let u = |v: u32, origin: u32, len: usize| {
                    (v as f32 / 2.0 - (origin * TILE) as f32) / len as f32
                };
                let uv = ERect::from_min_max(
                    pos2(u(r.x, ptx, cols), u(r.y, pty, rows)),
                    pos2(u(r.x + r.w, ptx, cols), u(r.y + r.h, pty, rows)),
                );
                draws.push(TileDraw {
                    keys: std::array::from_fn(|c| parents[ch(c)].clone()),
                    tiles: std::array::from_fn(|c| tiles[ch(c)].clone()),
                    no_data,
                    screen: screen_rect,
                    uv,
                });
            }
        }
    }

    // Mask overlays: ready tiles at the image's level only (no coarser fallback).
    let mut overlays = Vec::new();
    for (i, (name, src, color)) in masks.iter().enumerate() {
        if src.levels() <= level || src.size() != (w, h) {
            continue;
        }
        let mut draws = Vec::new();
        for (tx, ty, r) in visible_tiles(view, rect, w, h, level) {
            let key = TileKey {
                product: view.product,
                band: name.clone(),
                level,
                tx,
                ty,
            };
            if let TileState::Ready(t) = state.cache.get(&key, src) {
                draws.push(TileDraw {
                    keys: std::array::from_fn(|_| key.clone()),
                    tiles: std::array::from_fn(|_| t.clone()),
                    no_data: [None; 3],
                    screen: ERect::from_min_max(screen(r.x, r.y), screen(r.x + r.w, r.y + r.h)),
                    uv: ERect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                });
            }
        }
        overlays.push((i, *color, draws));
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
            mode,
            color: [0.0; 4],
            lut: view.palette.to_lut(),
            draws,
        },
    ));
    for (i, color, draws) in overlays {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            CanvasCallback {
                id: view_idx as u64 | (i as u64 + 1) << 32,
                frame: ui.ctx().cumulative_pass_nr(),
                stretch: view.stretch.clone(),
                mode: 2,
                color,
                lut: [[0; 4]; 256],
                draws,
            },
        ));
    }
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
