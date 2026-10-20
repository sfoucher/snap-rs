#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Str(String),
    Int(i64),
    Float(f64),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetadataAttribute {
    pub name: String,
    pub value: MetadataValue,
    pub unit: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetadataElement {
    pub name: String,
    pub attributes: Vec<MetadataAttribute>,
    pub children: Vec<MetadataElement>,
}

impl MetadataElement {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn child(&self, name: &str) -> Option<&MetadataElement> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_mut_or_insert(&mut self, name: &str) -> &mut MetadataElement {
        let i = match self.children.iter().position(|c| c.name == name) {
            Some(i) => i,
            None => {
                self.children.push(MetadataElement::new(name));
                self.children.len() - 1
            }
        };
        &mut self.children[i]
    }

    pub fn attr(&self, name: &str) -> Option<&MetadataValue> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| &a.value)
    }

    /// `"A/B/attr"`: child elements then a final attribute name.
    pub fn get_path(&self, path: &str) -> Option<&MetadataValue> {
        let (dirs, attr) = match path.rsplit_once('/') {
            Some((d, a)) => (d, a),
            None => return self.attr(path),
        };
        let mut el = self;
        for seg in dirs.split('/') {
            el = el.child(seg)?;
        }
        el.attr(attr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_path_lookup() {
        let mut root = MetadataElement::new("root");
        root.child_mut_or_insert("TIFF")
            .attributes
            .push(MetadataAttribute {
                name: "Compression".into(),
                value: MetadataValue::Str("DEFLATE".into()),
                unit: None,
                description: None,
            });
        assert!(
            matches!(root.get_path("TIFF/Compression"), Some(MetadataValue::Str(s)) if s=="DEFLATE")
        );
        assert!(root.get_path("TIFF/Nope").is_none());
    }
}
