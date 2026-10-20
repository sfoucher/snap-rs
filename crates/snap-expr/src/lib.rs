//! SNAP band-maths expression parser (AST with source spans).
mod ast;
mod error;
mod parser;

pub use ast::{BinOp, Expr, Func, Span, UnOp};
pub use error::ExprError;
pub use parser::parse;
