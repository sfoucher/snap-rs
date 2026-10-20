use gdal::Dataset;
use gdal::Metadata;
use snap_core::{MetadataAttribute, MetadataElement, MetadataValue};

pub(crate) fn metadata_tree(ds: &Dataset) -> MetadataElement {
    let mut root = MetadataElement::new("metadata");
    for domain in ds.metadata_domains() {
        let name = if domain.is_empty() {
            "DEFAULT"
        } else {
            &domain
        };
        let el = root.child_mut_or_insert(name);
        for kv in ds.metadata_domain(&domain).unwrap_or_default() {
            if let Some((k, v)) = kv.split_once('=') {
                el.attributes.push(MetadataAttribute {
                    name: k.into(),
                    value: MetadataValue::Str(v.into()),
                    unit: None,
                    description: None,
                });
            }
        }
    }
    root
}
