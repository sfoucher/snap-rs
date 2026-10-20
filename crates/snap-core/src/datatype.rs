#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
}

impl DataType {
    pub fn byte_size(self) -> usize {
        match self {
            DataType::U8 | DataType::I8 => 1,
            DataType::U16 | DataType::I16 => 2,
            DataType::U32 | DataType::I32 | DataType::F32 => 4,
            DataType::F64 => 8,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, DataType::F32 | DataType::F64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scaling {
    pub factor: f64,
    pub offset: f64,
    pub log10: bool,
}

impl Default for Scaling {
    fn default() -> Self {
        Scaling {
            factor: 1.0,
            offset: 0.0,
            log10: false,
        }
    }
}
