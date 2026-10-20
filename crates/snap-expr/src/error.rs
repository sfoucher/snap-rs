use crate::ast::Span;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExprError {
    #[error("parse error at {}..{}: {msg}", span.start, span.end)]
    Parse { msg: String, span: Span },
    #[error("unknown function `{name}`")]
    UnknownFunction { name: String, span: Span },
    #[error("function `{name}` expects {expected} argument(s)")]
    Arity {
        name: String,
        expected: usize,
        span: Span,
    },
    #[error("unknown band or symbol `{name}`")]
    UnknownBand { name: String, span: Span },
    #[error("type mismatch: {msg}")]
    TypeMismatch { msg: String, span: Span },
}
