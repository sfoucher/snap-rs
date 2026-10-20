use crate::app::{AppState, NodeRef};

/// Masks of the active view's product (else the selected product).
pub fn mask_manager_ui(ui: &mut egui::Ui, state: &mut AppState) {
    let pid = state
        .active_view
        .and_then(|i| state.views.get(i))
        .map(|v| v.product)
        .or(state.selected.as_ref().map(NodeRef::product));
    let Some(prod) = pid.and_then(|id| state.product(id).cloned()) else {
        ui.label("Select a product or open an image view.");
        return;
    };
    let pid = pid.unwrap();
    let mut remove = None;
    egui::Grid::new("mask_grid").striped(true).show(ui, |ui| {
        for h in ["Visible", "Colour", "Opacity", "Name", "Expression"] {
            ui.strong(h);
        }
        ui.end_row();
        // Edit copies under a read lock; write back only when a widget changed.
        let masks: Vec<_> = prod
            .read()
            .unwrap()
            .masks
            .iter()
            .map(|m| (m.name.clone(), m.expr.clone(), m.color, m.opacity))
            .collect();
        for (name, expr, mut color, mut opacity) in masks {
            let m = (&name, &expr);
            let mut vis = state.mask_visible(pid, m.0);
            if ui.checkbox(&mut vis, "").changed() {
                state.set_mask_visible(pid, m.0, vis);
            }
            let c = ui
                .color_edit_button_srgba_unmultiplied(&mut color)
                .changed();
            let o = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0)).changed();
            if c || o {
                if let Some(mm) = prod
                    .write()
                    .unwrap()
                    .masks
                    .iter_mut()
                    .find(|x| x.name == name)
                {
                    mm.color = color;
                    mm.opacity = opacity;
                }
            }
            let node = NodeRef::Mask(pid, name.clone());
            if ui
                .selectable_label(state.selected.as_ref() == Some(&node), &name)
                .clicked()
            {
                state.selected = Some(node);
            }
            ui.label(&expr);
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Add…").clicked() {
            state.add_mask_for = Some(pid);
        }
        let sel = match &state.selected {
            Some(NodeRef::Mask(p, n)) if *p == pid => Some(n.clone()),
            _ => None,
        };
        if ui
            .add_enabled(sel.is_some(), egui::Button::new("Remove"))
            .clicked()
        {
            remove = sel;
        }
    });
    if let Some(name) = remove {
        state.remove_mask(pid, &name);
    }
}
