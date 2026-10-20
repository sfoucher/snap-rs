use crate::app::{AppState, NodeRef};
use snap_core::ProductId;

// Payloads of the variants not yet handled are read by the tasks that wire them.
#[allow(dead_code)]
pub enum ExplorerAction {
    OpenView(ProductId, String),
    OpenRgb(ProductId),
    BandMaths(ProductId),
    Remove(NodeRef),
    Export(ProductId),
    ShowProperties(NodeRef),
}

pub fn explorer_ui(ui: &mut egui::Ui, state: &mut AppState) -> Vec<ExplorerAction> {
    let mut actions = Vec::new();
    let products = state.products.clone();
    egui::ScrollArea::both().show(ui, |ui| {
        for product in &products {
            let p = product.read().unwrap();
            let id = p.id;
            let head = egui::CollapsingHeader::new(&p.name)
                .id_salt(("product", id.0))
                .default_open(true)
                .show(ui, |ui| {
                    egui::CollapsingHeader::new("Bands")
                        .id_salt(("bands", id.0))
                        .default_open(true)
                        .show(ui, |ui| {
                            for b in &p.bands {
                                let node = NodeRef::Band(id, b.name.clone());
                                let r = ui.selectable_label(
                                    state.selected.as_ref() == Some(&node),
                                    &b.name,
                                );
                                if r.double_clicked() {
                                    actions.push(ExplorerAction::OpenView(id, b.name.clone()));
                                }
                                r.context_menu(|ui| {
                                    if ui.button("Properties").clicked() {
                                        actions.push(ExplorerAction::ShowProperties(node.clone()));
                                        ui.close_menu();
                                    }
                                });
                                if r.clicked() {
                                    state.selected = Some(node);
                                }
                            }
                        });
                    if !p.masks.is_empty() {
                        egui::CollapsingHeader::new("Masks")
                            .id_salt(("masks", id.0))
                            .show(ui, |ui| {
                                for m in &p.masks {
                                    let node = NodeRef::Mask(id, m.name.clone());
                                    let on = state.selected.as_ref() == Some(&node);
                                    if ui.selectable_label(on, &m.name).clicked() {
                                        state.selected = Some(node);
                                    }
                                }
                            });
                    }
                    let node = NodeRef::Metadata(id);
                    let on = state.selected.as_ref() == Some(&node);
                    if ui.selectable_label(on, "Metadata").clicked() {
                        state.selected = Some(node);
                    }
                });
            let h = head.header_response;
            if h.clicked() {
                state.selected = Some(NodeRef::Product(id));
            }
            h.context_menu(|ui| {
                let node = NodeRef::Product(id);
                if ui.button("Properties").clicked() {
                    actions.push(ExplorerAction::ShowProperties(node.clone()));
                }
                if ui.button("Open RGB Image View").clicked() {
                    actions.push(ExplorerAction::OpenRgb(id));
                }
                if ui.button("Band Maths…").clicked() {
                    actions.push(ExplorerAction::BandMaths(id));
                }
                if ui.button("Export Subset…").clicked() {
                    actions.push(ExplorerAction::Export(id));
                }
                if ui.button("Remove").clicked() {
                    actions.push(ExplorerAction::Remove(node));
                }
                if !actions.is_empty() {
                    ui.close_menu();
                }
            });
        }
    });
    actions
}
