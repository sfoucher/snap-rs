use crate::app::AppState;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke};
use snap_core::{Band, DataType, Mask, Product, ProductId, RasterSource, Scaling};
use snap_expr::{compile, parse, ExprError, ExprSource, Span};
use std::ops::Range;
use std::sync::{Arc, OnceLock};

pub struct BandMathsRequest {
    pub product: ProductId,
    pub name: String,
    pub expr: String,
    pub as_mask: bool,
    pub color: [u8; 4],
    pub description: Option<String>,
}

/// Names are unique across a product's bands and masks (R29).
fn name_taken(p: &Product, name: &str) -> bool {
    p.band(name).is_some() || p.masks.iter().any(|m| m.name == name)
}

/// `new_band_N` for the smallest N >= 1 that `taken` does not claim.
fn default_name(taken: impl Fn(&str) -> bool) -> String {
    (1..)
        .map(|n| format!("new_band_{n}"))
        .find(|n| !taken(n))
        .unwrap()
}

pub fn apply_band_maths(state: &mut AppState, req: &BandMathsRequest) -> Result<(), ExprError> {
    let whole = Span {
        start: 0,
        end: req.expr.len(),
    };
    let err = |msg: String, span| ExprError::Parse { msg, span };
    let prod = state
        .product(req.product)
        .ok_or_else(|| err("product is closed".into(), whole))?
        .clone();
    let mut p = prod.write().unwrap();
    if name_taken(&p, &req.name) {
        let msg = format!("name '{}' is already used", req.name);
        return Err(err(msg, Span { start: 0, end: 0 }));
    }
    let source = ExprSource::new(&req.expr, &p)?;
    if req.as_mask {
        if source.data_type() != DataType::U8 {
            let msg = "a mask needs a boolean expression".into();
            return Err(ExprError::TypeMismatch { msg, span: whole });
        }
        p.masks.push(Mask {
            name: req.name.clone(),
            expr: req.expr.clone(),
            color: req.color,
            opacity: 0.5,
            source: Arc::new(source),
        });
        return Ok(());
    }
    let (width, height) = source.size();
    p.add_band(Band {
        name: req.name.clone(),
        data_type: DataType::F32,
        width,
        height,
        no_data: None,
        scaling: Scaling::default(),
        unit: None,
        valid_pixel_expr: None,
        description: req.description.clone(),
        source: Arc::new(source),
        stats: OnceLock::new(),
    })
    .map_err(|e| err(e.to_string(), whole))
}

#[derive(Clone, Default)]
struct Dlg {
    name: String,
    expr: String,
    as_mask: bool,
    color: [u8; 4],
    desc: String,
}

impl Dlg {
    fn new(p: &Product, as_mask: bool) -> Self {
        Dlg {
            name: default_name(|n| name_taken(p, n)),
            expr: String::new(),
            as_mask,
            color: [255, 0, 0, 255],
            desc: String::new(),
        }
    }
}

fn dlg_id() -> egui::Id {
    egui::Id::new("band_maths_dialog")
}

/// Resets the dialog's form, with "Create as mask" preset.
pub fn preset_as_mask(ctx: &egui::Context, state: &AppState, product: ProductId) {
    if let Some(p) = state.product(product) {
        let d = Dlg::new(&p.read().unwrap(), true);
        ctx.data_mut(|m| m.insert_temp(dlg_id(), d));
    }
}

/// Byte range of `text` to underline for `span`: clamped to the text and to char
/// boundaries; an empty span marks the char it points at (or the last char).
fn underline_range(text: &str, span: Span) -> Option<Range<usize>> {
    if text.is_empty() {
        return None;
    }
    let floor = |i: usize| {
        (0..=i.min(text.len()))
            .rev()
            .find(|&i| text.is_char_boundary(i))
    };
    let (mut s, mut e) = (floor(span.start)?, floor(span.end)?);
    if e <= s {
        if s >= text.len() {
            s = text.char_indices().last()?.0;
        }
        e = s + text[s..].chars().next()?.len_utf8();
    }
    Some(s..e)
}

fn layout_job(text: &str, bad: Option<Range<usize>>, font: FontId, color: Color32) -> LayoutJob {
    let fmt = |underline| TextFormat {
        font_id: font.clone(),
        color,
        underline,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    match bad {
        Some(r) => {
            job.append(&text[..r.start], 0.0, fmt(Stroke::NONE));
            job.append(
                &text[r.clone()],
                0.0,
                fmt(Stroke::new(1.5_f32, Color32::LIGHT_RED)),
            );
            job.append(&text[r.end..], 0.0, fmt(Stroke::NONE));
        }
        None => job.append(text, 0.0, fmt(Stroke::NONE)),
    }
    job
}

/// Parses and compiles `src` against `bands`; Ok(is_boolean).
fn check(src: &str, bands: &[String]) -> Result<bool, ExprError> {
    Ok(compile(&parse(src)?, bands)?.is_boolean)
}

fn err_span(e: &ExprError) -> Span {
    match e {
        ExprError::Parse { span, .. }
        | ExprError::UnknownFunction { span, .. }
        | ExprError::Arity { span, .. }
        | ExprError::UnknownBand { span, .. }
        | ExprError::TypeMismatch { span, .. } => *span,
    }
}

pub fn band_maths_dialog(
    ctx: &egui::Context,
    state: &mut AppState,
    product: ProductId,
    open: &mut bool,
) -> Option<BandMathsRequest> {
    let Some(prod) = state.product(product).cloned() else {
        *open = false;
        return None;
    };
    let (bands, mut d, taken) = {
        let p = prod.read().unwrap();
        let bands: Vec<String> = p.bands.iter().map(|b| b.name.clone()).collect();
        let d: Dlg = ctx
            .data(|m| m.get_temp(dlg_id()))
            .unwrap_or_else(|| Dlg::new(&p, false));
        let taken = name_taken(&p, &d.name);
        (bands, d, taken)
    };
    let mut result = None;
    let mut close = false;
    let mut keep = true;
    egui::Window::new("Band Maths")
        .open(&mut keep)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut d.name);
            });
            if d.name.trim().is_empty() {
                ui.colored_label(Color32::LIGHT_RED, "name is empty");
            } else if taken {
                ui.colored_label(
                    Color32::LIGHT_RED,
                    format!("name '{}' is already used", d.name),
                );
            }
            ui.horizontal(|ui| {
                ui.label("Description");
                ui.text_edit_singleline(&mut d.desc);
            });
            ui.checkbox(&mut d.as_mask, "Create as mask");
            if d.as_mask {
                ui.horizontal(|ui| {
                    ui.label("Colour");
                    ui.color_edit_button_srgba_unmultiplied(&mut d.color);
                });
            }
            let mut res = check(&d.expr, &bands);
            if d.as_mask && matches!(res, Ok(false)) {
                res = Err(ExprError::TypeMismatch {
                    msg: "a mask needs a boolean expression".into(),
                    span: Span {
                        start: 0,
                        end: d.expr.len(),
                    },
                });
            }
            let bad = res
                .as_ref()
                .err()
                .and_then(|e| underline_range(&d.expr, err_span(e)));
            let (font, color) = (
                egui::TextStyle::Monospace.resolve(ui.style()),
                ui.visuals().text_color(),
            );
            let mut layouter = |ui: &egui::Ui, text: &str, wrap: f32| {
                let mut job = layout_job(text, bad.clone(), font.clone(), color);
                job.wrap.max_width = wrap;
                ui.fonts(|f| f.layout_job(job))
            };
            let te_id = dlg_id().with("expr");
            ui.label("Expression");
            ui.add(
                egui::TextEdit::multiline(&mut d.expr)
                    .id(te_id)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            );
            if let Err(e) = &res {
                if !d.expr.trim().is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, e.to_string());
                }
            }
            ui.label("Insert band");
            egui::ScrollArea::vertical()
                .max_height(90.0)
                .id_salt("bm_bands")
                .show(ui, |ui| {
                    for b in &bands {
                        if ui.selectable_label(false, b).clicked() {
                            let mut st = egui::TextEdit::load_state(ctx, te_id).unwrap_or_default();
                            let chars = d.expr.chars().count();
                            let at = st
                                .cursor
                                .char_range()
                                .map_or(chars, |r| r.primary.index.min(chars));
                            let byte = d.expr.char_indices().nth(at).map_or(d.expr.len(), |c| c.0);
                            d.expr.insert_str(byte, b);
                            let end = egui::text::CCursor::new(at + b.chars().count());
                            st.cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(end)));
                            st.store(ctx, te_id);
                        }
                    }
                });
            ui.horizontal(|ui| {
                let ok = res.is_ok() && !taken && !d.name.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("OK")).clicked() {
                    result = Some(BandMathsRequest {
                        product,
                        name: d.name.trim().to_string(),
                        expr: d.expr.clone(),
                        as_mask: d.as_mask,
                        color: d.color,
                        description: Some(d.desc.trim().to_string()).filter(|s| !s.is_empty()),
                    });
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if result.is_some() || close || !keep {
        *open = false;
        ctx.data_mut(|m| m.remove_temp::<Dlg>(dlg_id()));
    } else {
        ctx.data_mut(|m| m.insert_temp(dlg_id(), d));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fixtures;
    use snap_core::DataType;
    use snap_expr::Span;

    fn setup(tag: &str) -> (AppState, ProductId, std::path::PathBuf) {
        snap_io_gdal::init_error_handler();
        let dir = fixtures::temp_dir(tag);
        let fx = fixtures::make_fixture(
            &dir,
            "a.tif",
            64,
            64,
            DataType::U16,
            2,
            false,
            None,
            Some(32633),
        );
        let mut st = AppState::new();
        let id = st.open_path(&fx.path).unwrap();
        (st, id, dir)
    }

    fn req(pid: ProductId, name: &str, expr: &str, as_mask: bool) -> BandMathsRequest {
        BandMathsRequest {
            product: pid,
            name: name.into(),
            expr: expr.into(),
            as_mask,
            color: [255, 0, 0, 255],
            description: None,
        }
    }

    #[test]
    fn band_maths_creates_virtual_band() {
        let (mut st, id, dir) = setup("bm-band");
        apply_band_maths(
            &mut st,
            &req(id, "ndvi", "(band_2 - band_1) / (band_2 + band_1)", false),
        )
        .unwrap();
        let p = st.product(id).unwrap().read().unwrap();
        let b = p.band("ndvi").unwrap();
        assert_eq!(b.data_type, DataType::F32);
        let t = b.source.read_tile(0, 0, 0).unwrap();
        // band_1 = 0, band_2 = 100 at (0,0) (R17)
        assert_eq!(t.get_f64(0, 0), 1.0);
        drop(p);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn band_maths_as_mask_creates_mask() {
        let (mut st, id, dir) = setup("bm-mask");
        apply_band_maths(&mut st, &req(id, "hi", "band_1 > 500", true)).unwrap();
        let p = st.product(id).unwrap().read().unwrap();
        assert_eq!(p.masks.len(), 1);
        assert_eq!(p.masks[0].source.data_type(), DataType::U8);
        assert_eq!(p.masks[0].expr, "band_1 > 500");
        drop(p);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn band_maths_bad_expr_returns_span() {
        let (mut st, id, dir) = setup("bm-bad");
        let n = st.product(id).unwrap().read().unwrap().bands.len();
        let e = apply_band_maths(&mut st, &req(id, "x", "band_99 + 1", false)).unwrap_err();
        assert!(
            matches!(
                e,
                ExprError::UnknownBand {
                    span: Span { start: 0, end: 7 },
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(st.product(id).unwrap().read().unwrap().bands.len(), n);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn band_maths_rejects_taken_names_and_non_boolean_masks() {
        let (mut st, id, dir) = setup("bm-rules");
        apply_band_maths(&mut st, &req(id, "m", "band_1 > 5", true)).unwrap();
        for name in ["band_1", "m"] {
            let e = apply_band_maths(&mut st, &req(id, name, "band_1 + 1", false)).unwrap_err();
            assert!(matches!(e, ExprError::Parse { .. }), "{e:?}");
        }
        let e = apply_band_maths(&mut st, &req(id, "n", "band_1 + 1", true)).unwrap_err();
        assert!(matches!(e, ExprError::TypeMismatch { .. }), "{e:?}");
        let p = st.product(id).unwrap().read().unwrap();
        assert_eq!((p.bands.len(), p.masks.len()), (2, 1));
        drop(p);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mask_visibility_defaults_on_and_remove_evicts_tiles() {
        let (mut st, id, dir) = setup("bm-vis");
        apply_band_maths(&mut st, &req(id, "m", "band_1 > 5", true)).unwrap();
        assert!(st.mask_visible(id, "m"));
        st.set_mask_visible(id, "m", false);
        assert!(!st.mask_visible(id, "m"));
        let src = st.product(id).unwrap().read().unwrap().masks[0]
            .source
            .clone();
        let key = snap_core::TileKey {
            product: id,
            band: "m".into(),
            level: 0,
            tx: 0,
            ty: 0,
        };
        st.cache.get_blocking(&key, &src).unwrap();
        assert!(st.cache.peek(&key).is_some());
        st.remove_mask(id, "m");
        assert!(st.cache.peek(&key).is_none());
        assert!(st.product(id).unwrap().read().unwrap().masks.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn default_name_skips_taken() {
        let taken = |n: &str| n == "new_band_1" || n == "new_band_2";
        assert_eq!(default_name(taken), "new_band_3");
        assert_eq!(default_name(|_| false), "new_band_1");
    }

    #[test]
    fn underline_range_clamps() {
        let sp = |start, end| Span { start, end };
        assert_eq!(underline_range("band_99 + 1", sp(0, 7)), Some(0..7));
        assert_eq!(underline_range("a +", sp(3, 3)), Some(2..3));
        assert_eq!(underline_range("ab", sp(1, 99)), Some(1..2));
        assert_eq!(underline_range("é+", sp(1, 2)), Some(0..2));
        assert_eq!(underline_range("", sp(0, 0)), None);
    }
}
