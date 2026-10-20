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
        let mut p = prod.write().unwrap();
        for m in &mut p.masks {
            let mut vis = state.mask_visible(pid, &m.name);
            if ui.checkbox(&mut vis, "").changed() {
                state.set_mask_visible(pid, &m.name, vis);
            }
            ui.color_edit_button_srgba_unmultiplied(&mut m.color);
            ui.add(egui::Slider::new(&mut m.opacity, 0.0..=1.0));
            let node = NodeRef::Mask(pid, m.name.clone());
            if ui
                .selectable_label(state.selected.as_ref() == Some(&node), &m.name)
                .clicked()
            {
                state.selected = Some(node);
            }
            ui.label(&m.expr);
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
