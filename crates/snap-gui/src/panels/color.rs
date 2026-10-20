use crate::app::AppState;
use crate::palette_cpd::{builtin_palettes, parse_cpd};
use crate::view::{Stretch, ViewSources};
use egui_plot::{Bar, BarChart, Plot, VLine};
use snap_core::BandStats;

/// Percentile 2-98 window; never degenerate (a flat band gets a 1-unit window).
pub fn default_stretch(stats: &BandStats) -> Stretch {
    if stats.sample_count == 0 {
        return Stretch {
            min: 0.0,
            max: 1.0,
            gamma: 1.0,
        };
    }
    let (mut lo, mut hi) = (stats.percentile(2.0), stats.percentile(98.0));
    if lo >= hi {
        (lo, hi) = (stats.min, stats.max);
    }
    if lo >= hi {
        (lo, hi) = (lo - 0.5, lo + 0.5);
    }
    Stretch {
        min: lo as f32,
        max: hi as f32,
        gamma: 1.0,
    }
}

fn histogram(ui: &mut egui::Ui, id: egui::Id, stats: &BandStats, s: &mut Stretch) {
    let bars = stats
        .histogram
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            Bar::new(stats.min + (i as f64 + 0.5) * stats.bin_width, c as f64)
                .width(stats.bin_width)
        })
        .collect();
    let grab = id.with("grab");
    let (mut min, mut max) = (s.min as f64, s.max as f64);
    Plot::new(id)
        .height(100.0)
        .show_axes([true, false])
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_drag(false)
        .show(ui, |pu| {
            pu.bar_chart(BarChart::new(bars));
            pu.vline(VLine::new(min));
            pu.vline(VLine::new(max));
            let r = pu.response().clone();
            let Some(x) = pu.pointer_coordinate().map(|p| p.x) else {
                return;
            };
            if r.drag_started() {
                // true = min line, the nearer of the two to the pointer
                let near_min = (x - min).abs() <= (x - max).abs();
                pu.ctx().data_mut(|d| d.insert_temp(grab, near_min));
            }
            if r.dragged() {
                match pu.ctx().data(|d| d.get_temp::<bool>(grab)) {
                    Some(true) => min = x,
                    Some(false) => max = x,
                    None => {}
                }
            }
        });
    (s.min, s.max) = (min as f32, max as f32);
}

fn channel_ui(
    ui: &mut egui::Ui,
    id: egui::Id,
    stats: Option<&BandStats>,
    dt_stretch: &Stretch,
    s: &mut Stretch,
) {
    match stats {
        Some(st) if st.sample_count > 0 => histogram(ui, id, st, s),
        Some(_) => {
            ui.label("no valid samples");
        }
        None => {
            ui.label("computing…");
        }
    }
    let speed = 0.01 * (s.max - s.min).abs().max(1e-3) as f64;
    ui.horizontal(|ui| {
        ui.label("Min");
        ui.add(egui::DragValue::new(&mut s.min).speed(speed));
        ui.label("Max");
        ui.add(egui::DragValue::new(&mut s.max).speed(speed));
    });
    ui.horizontal(|ui| {
        let valid = stats.filter(|st| st.sample_count > 0);
        if ui
            .add_enabled(valid.is_some(), egui::Button::new("2–98 %"))
            .clicked()
        {
            *s = Stretch {
                gamma: s.gamma,
                ..default_stretch(valid.unwrap())
            };
        }
        if ui
            .add_enabled(valid.is_some(), egui::Button::new("Min/Max"))
            .clicked()
        {
            let st = valid.unwrap();
            let (lo, hi) = (st.min as f32, st.max as f32);
            (s.min, s.max) = if lo < hi {
                (lo, hi)
            } else {
                (lo - 0.5, lo + 0.5)
            };
        }
        if ui.button("Reset").clicked() {
            *s = valid.map_or_else(|| dt_stretch.clone(), default_stretch);
        }
    });
    ui.add(egui::Slider::new(&mut s.gamma, 0.1..=5.0).text("Gamma"));
}

pub fn color_panel_ui(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(vi) = state.active_view.filter(|&i| i < state.views.len()) else {
        ui.label("No active view");
        return;
    };
    let view = &state.views[vi];
    let names: Vec<String> = match &view.sources {
        ViewSources::Single(n) => vec![n.clone()],
        ViewSources::Rgb(n) => n.to_vec(),
    };
    let rgb = names.len() == 3;
    // (stats clone, data-type range) per channel; the lock is held only for this.
    let info: Vec<(Option<BandStats>, Option<Stretch>)> = {
        let prod = state.product(view.product).map(|p| p.read().unwrap());
        names
            .iter()
            .map(|n| {
                let b = prod.as_ref().and_then(|p| p.band(n));
                (
                    b.and_then(|b| b.stats.get().cloned()),
                    b.map(|b| Stretch::for_type(b.data_type)),
                )
            })
            .collect()
    };
    let sync_id = egui::Id::new("color_sync_rgb");
    let mut sync = ui.data(|d| d.get_temp::<bool>(sync_id)).unwrap_or(true);
    if rgb {
        ui.checkbox(&mut sync, "Sync RGB sliders");
        ui.data_mut(|d| d.insert_temp(sync_id, sync));
    }
    let mut errors = Vec::new();
    let view = &mut state.views[vi];
    for (ch, name) in names.iter().enumerate() {
        ui.separator();
        ui.strong(name);
        let before = view.stretch[ch].clone();
        let mut s = before.clone();
        let (stats, dt) = &info[ch];
        let dt = dt.clone().unwrap_or(Stretch {
            min: 0.0,
            max: 1.0,
            gamma: 1.0,
        });
        channel_ui(
            ui,
            egui::Id::new(("color_hist", vi, ch)),
            stats.as_ref(),
            &dt,
            &mut s,
        );
        if s != before {
            if rgb && sync {
                view.stretch = [s.clone(), s.clone(), s];
            } else {
                view.stretch[ch] = s;
            }
        }
    }
    if !rgb {
        ui.separator();
        let mut chosen = None;
        egui::ComboBox::from_label("Palette")
            .selected_text(view.palette.name.clone())
            .show_ui(ui, |ui| {
                for p in builtin_palettes() {
                    if ui
                        .selectable_label(view.palette.name == p.name, &p.name)
                        .clicked()
                    {
                        chosen = Some(p);
                    }
                }
            });
        if ui.button("Load .cpd…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("SNAP palette", &["cpd"])
                .pick_file()
            {
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| parse_cpd(&t))
                {
                    Ok(mut p) => {
                        if let Some(stem) = path.file_stem() {
                            p.name = stem.to_string_lossy().into_owned();
                        }
                        chosen = Some(p);
                    }
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }
        }
        if let Some(p) = chosen {
            view.palette = p;
        }
    }
    state.errors.extend(errors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;
    use snap_core::{compute_stats, MemorySource, RasterSource};
    use std::sync::Arc;

    fn stats(a: Array2<f32>) -> BandStats {
        let s: Arc<dyn RasterSource> = Arc::new(MemorySource::new(a));
        compute_stats(&s, None, 0).unwrap()
    }

    #[test]
    fn default_stretch_uses_percentiles() {
        let s = default_stretch(&stats(Array2::from_shape_fn((1, 1000), |(_, x)| x as f32)));
        assert!((s.min - 20.0).abs() <= 2.0, "{s:?}");
        assert!((s.max - 980.0).abs() <= 2.0, "{s:?}");
        assert_eq!(s.gamma, 1.0);
    }

    #[test]
    fn default_stretch_constant_band_is_not_degenerate() {
        let s = default_stretch(&stats(Array2::from_elem((4, 4), 7.0)));
        assert_eq!(
            s,
            Stretch {
                min: 6.5,
                max: 7.5,
                gamma: 1.0
            }
        );
    }
}
