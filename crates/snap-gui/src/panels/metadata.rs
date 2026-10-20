use snap_core::{MetadataAttribute, MetadataElement, MetadataValue};

/// Attribute count above which an element's rows are virtualised in their own scroll area.
const MANY: usize = 50;
const MANY_HEIGHT: f32 = 300.0;

fn value_text(v: &MetadataValue) -> String {
    let join = |it: Vec<String>| it.join(", ");
    match v {
        MetadataValue::Str(s) => s.clone(),
        MetadataValue::Int(i) => i.to_string(),
        MetadataValue::Float(f) => f.to_string(),
        MetadataValue::Ints(v) => join(v.iter().map(|x| x.to_string()).collect()),
        MetadataValue::Floats(v) => join(v.iter().map(|x| x.to_string()).collect()),
    }
}

/// Depth-first (path, value) rows; the root's own name is not part of the path.
/// `filter` is a case-insensitive substring of the path or the value.
pub fn flatten_filtered(root: &MetadataElement, filter: &str) -> Vec<(String, String)> {
    fn walk(e: &MetadataElement, prefix: &str, f: &str, out: &mut Vec<(String, String)>) {
        for a in &e.attributes {
            let path = format!("{prefix}{}", a.name);
            let value = value_text(&a.value);
            if f.is_empty() || path.to_lowercase().contains(f) || value.to_lowercase().contains(f) {
                out.push((path, value));
            }
        }
        for c in &e.children {
            walk(c, &format!("{prefix}{}/", c.name), f, out);
        }
    }
    let mut out = Vec::new();
    walk(root, "", &filter.to_lowercase(), &mut out);
    out
}

fn attr_row(ui: &mut egui::Ui, a: &MetadataAttribute) {
    ui.label(&a.name);
    let mut text = value_text(&a.value);
    if let Some(u) = &a.unit {
        text = format!("{text} {u}");
    }
    let r = ui.label(text);
    if let Some(d) = &a.description {
        r.on_hover_text(d);
    }
}

fn element_ui(ui: &mut egui::Ui, e: &MetadataElement, path: &str) {
    let path = format!("{path}/{}", e.name);
    egui::CollapsingHeader::new(&e.name)
        .id_salt(&path)
        .show(ui, |ui| {
            if e.attributes.len() > MANY {
                let h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
                egui::ScrollArea::vertical()
                    .id_salt(("rows", &path))
                    .max_height(MANY_HEIGHT)
                    .show_rows(ui, h, e.attributes.len(), |ui, range| {
                        for a in &e.attributes[range] {
                            ui.horizontal(|ui| attr_row(ui, a));
                        }
                    });
            } else if !e.attributes.is_empty() {
                egui::Grid::new(("grid", &path))
                    .striped(true)
                    .show(ui, |ui| {
                        for a in &e.attributes {
                            attr_row(ui, a);
                            ui.end_row();
                        }
                    });
            }
            for c in &e.children {
                element_ui(ui, c, &path);
            }
        });
}

pub fn metadata_ui(ui: &mut egui::Ui, root: &MetadataElement, filter: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.text_edit_singleline(filter);
    });
    if filter.is_empty() {
        element_ui(ui, root, "");
        return;
    }
    let rows = flatten_filtered(root, filter);
    let h = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical()
        .id_salt("metadata-filtered")
        .show_rows(ui, h, rows.len(), |ui, range| {
            for (path, value) in &rows[range] {
                ui.horizontal(|ui| {
                    ui.label(path);
                    ui.label(value);
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use snap_core::MetadataAttribute;

    fn attr(name: &str, value: MetadataValue) -> MetadataAttribute {
        MetadataAttribute {
            name: name.into(),
            value,
            unit: None,
            description: None,
        }
    }

    fn tree() -> MetadataElement {
        MetadataElement {
            name: "root".into(),
            attributes: vec![attr("a", MetadataValue::Int(7))],
            children: vec![MetadataElement {
                name: "TIFF".into(),
                attributes: vec![
                    attr("Compression", MetadataValue::Str("DEFLATE".into())),
                    attr("Res", MetadataValue::Floats(vec![1.5, 2.0])),
                ],
                children: vec![],
            }],
        }
    }

    #[test]
    fn flatten_filter_matches_value_and_path() {
        let rows = flatten_filtered(&tree(), "defl");
        assert_eq!(
            rows,
            vec![("TIFF/Compression".to_string(), "DEFLATE".to_string())]
        );
        // path match, case-insensitive
        assert_eq!(flatten_filtered(&tree(), "tiff/res").len(), 1);
    }

    #[test]
    fn flatten_empty_filter_returns_all_rows_without_root_name() {
        let rows = flatten_filtered(&tree(), "");
        let paths: Vec<_> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(paths, ["a", "TIFF/Compression", "TIFF/Res"]);
        assert_eq!(rows[0].1, "7");
        assert_eq!(rows[2].1, "1.5, 2");
    }
}
