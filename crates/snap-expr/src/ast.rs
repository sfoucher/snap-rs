#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Sqrt,
    Exp,
    Log,
    Log10,
    Abs,
    Min,
    Max,
    Pow,
    Sin,
    Cos,
    Tan,
    Nan,
    Inf,
    Floor,
    Ceil,
    Round,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64, Span),
    /// PI, E or NaN
    Const(&'static str, Span),
    Ident(String, Span),
    Unary(UnOp, Box<Expr>, Span),
    Binary(BinOp, Box<Expr>, Box<Expr>, Span),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>, Span),
    Call(Func, Vec<Expr>, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Num(_, s)
            | Expr::Const(_, s)
            | Expr::Ident(_, s)
            | Expr::Unary(_, _, s)
            | Expr::Binary(_, _, _, s)
            | Expr::Ternary(_, _, _, s)
            | Expr::Call(_, _, s) => *s,
        }
    }

    /// Identifiers in source order, duplicates included.
    pub fn identifiers(&self) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<(String, Span)>) {
        match self {
            Expr::Num(..) | Expr::Const(..) => {}
            Expr::Ident(n, s) => out.push((n.clone(), *s)),
            Expr::Unary(_, a, _) => a.collect(out),
            Expr::Binary(_, a, b, _) => {
                a.collect(out);
                b.collect(out);
            }
            Expr::Ternary(a, b, c, _) => {
                a.collect(out);
                b.collect(out);
                c.collect(out);
            }
            Expr::Call(_, args, _) => args.iter().for_each(|a| a.collect(out)),
        }
    }
}
