use crate::app::AppState;
use snap_core::ProductId;

pub const PRESETS: [(&str, [&str; 3]); 2] = [
    ("Natural colour", ["B4", "B3", "B2"]),
    ("False colour (NIR)", ["B8", "B4", "B3"]),
];

pub fn default_rgb_bands(band_names: &[String]) -> [String; 3] {
    let want = ["band_3", "band_2", "band_1"];
    if want.iter().all(|w| band_names.iter().any(|b| b == w)) {
        return want.map(String::from);
    }
    let last = band_names.last().cloned().unwrap_or_default();
    std::array::from_fn(|i| band_names.get(i).unwrap_or(&last).clone())
}

/// First preset whose three patterns each match a band (an exact name beats a substring,
/// so "B8" picks B8 over B8A).
pub fn match_preset<'a>(
    band_names: &[String],
    presets: &[(&'a str, [&str; 3])],
) -> Option<(&'a str, [String; 3])> {
    let find = |pat: &str| {
        band_names
            .iter()
            .find(|b| *b == pat)
            .or_else(|| band_names.iter().find(|b| b.contains(pat)))
    };
    presets.iter().find_map(|(name, pats)| {
        let [r, g, b] = pats.map(find);
        Some((*name, [r?.clone(), g?.clone(), b?.clone()]))
    })
}

/// Modal-style window with a band combo per channel; Some(bands) when OK is pressed.
pub fn rgb_dialog(
    ctx: &egui::Context,
    state: &mut AppState,
    product: ProductId,
    open: &mut bool,
) -> Option<[String; 3]> {
    let names: Vec<String> = state
        .product(product)?
        .read()
        .unwrap()
        .bands
        .iter()
        .map(|b| b.name.clone())
        .collect();
    let id = egui::Id::new(("rgb-dialog", product.0));
    let mut sel: [String; 3] = ctx
        .data_mut(|d| d.get_temp(id))
        .unwrap_or_else(|| default_rgb_bands(&names));
    let (mut ok, mut cancel) = (false, false);
    egui::Window::new("Select RGB Image Channels")
        .collapsible(false)
        .show(ctx, |ui| {
            for (label, s) in ["Red", "Green", "Blue"].into_iter().zip(&mut sel) {
                egui::ComboBox::from_label(label)
                    .selected_text(s.as_str())
                    .show_ui(ui, |ui| {
                        for n in &names {
                            ui.selectable_value(s, n.clone(), n);
                        }
                    });
            }
            ui.horizontal(|ui| {
                for (name, pats) in PRESETS {
                    if let Some((_, m)) = match_preset(&names, &[(name, pats)]) {
                        if ui.button(name).clicked() {
                            sel = m;
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ok = ui.button("OK").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    if ok || cancel {
        *open = false;
        ctx.data_mut(|d| d.remove_temp::<[String; 3]>(id));
        return ok.then_some(sel);
    }
    ctx.data_mut(|d| d.insert_temp(id, sel));
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rgb_default_selection() {
        let b = names(&["band_1", "band_2", "band_3", "band_4"]);
        assert_eq!(default_rgb_bands(&b), ["band_3", "band_2", "band_1"]);
        let s = names(&["B2", "B3", "B4", "B8"]);
        assert_eq!(
            match_preset(&s, &PRESETS),
            Some(("Natural colour", ["B4", "B3", "B2"].map(String::from)))
        );
        assert_eq!(match_preset(&b, &PRESETS), None);
    }

    #[test]
    fn default_falls_back_to_first_three_then_repeats_last() {
        assert_eq!(
            default_rgb_bands(&names(&["x", "y", "z", "w"])),
            ["x", "y", "z"]
        );
        assert_eq!(default_rgb_bands(&names(&["a", "b"])), ["a", "b", "b"]);
        assert_eq!(default_rgb_bands(&names(&["a"])), ["a", "a", "a"]);
    }

    #[test]
    fn preset_prefers_exact_over_substring() {
        let s = names(&["B8A", "B8", "B4", "B3", "B2"]);
        assert_eq!(
            match_preset(&s, &PRESETS[1..]).unwrap().1,
            ["B8", "B4", "B3"]
        );
    }
}
