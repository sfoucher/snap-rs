//! SNAP band-maths expressions: parser (AST with source spans), evaluator and `ExprSource`.
mod ast;
mod error;
mod eval;
mod parser;
mod source;

pub use ast::{BinOp, Expr, Func, Span, UnOp};
pub use error::ExprError;
pub use eval::{compile, Compiled, PixelCtx};
pub use parser::parse;
pub use source::ExprSource;
