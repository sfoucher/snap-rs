use chumsky::prelude::*;
use chumsky::Stream;

use crate::ast::{BinOp, Expr, Func, Span, UnOp};
use crate::error::ExprError;

type P<'a, T> = BoxedParser<'a, char, T, Simple<char>>;

const FUNCS: [(&str, Func, usize); 16] = [
    ("sqrt", Func::Sqrt, 1),
    ("exp", Func::Exp, 1),
    ("log", Func::Log, 1),
    ("log10", Func::Log10, 1),
    ("abs", Func::Abs, 1),
    ("min", Func::Min, 2),
    ("max", Func::Max, 2),
    ("pow", Func::Pow, 2),
    ("sin", Func::Sin, 1),
    ("cos", Func::Cos, 1),
    ("tan", Func::Tan, 1),
    ("nan", Func::Nan, 1),
    ("inf", Func::Inf, 1),
    ("floor", Func::Floor, 1),
    ("ceil", Func::Ceil, 1),
    ("round", Func::Round, 1),
];

fn sp(r: std::ops::Range<usize>) -> Span {
    Span {
        start: r.start,
        end: r.end,
    }
}

fn join(a: &Expr, b: &Expr) -> Span {
    Span {
        start: a.span().start,
        end: b.span().end,
    }
}

/// Left-assoc binary level: `next (op next)*`.
fn level<'a>(next: P<'a, Expr>, ops: P<'a, BinOp>) -> P<'a, Expr> {
    next.clone()
        .then(ops.padded().then(next).repeated())
        .foldl(|l, (op, r)| {
            let s = join(&l, &r);
            Expr::Binary(op, Box::new(l), Box::new(r), s)
        })
        .boxed()
}

fn op<'a>(tok: &'static str, b: BinOp) -> P<'a, BinOp> {
    just(tok).to(b).boxed()
}

fn expr_parser<'a>() -> P<'a, Expr> {
    recursive(|expr: Recursive<'a, char, Expr, Simple<char>>| {
        let digit = || filter(|c: &char| c.is_ascii_digit());
        let mantissa = digit()
            .repeated()
            .at_least(1)
            .chain::<char, _, _>(
                just('.')
                    .chain::<char, _, _>(digit().repeated())
                    .or_not()
                    .map(Option::unwrap_or_default),
            )
            .or(just('.').chain(digit().repeated().at_least(1)));
        let exp = one_of("eE")
            .chain::<char, _, _>(one_of("+-").or_not())
            .chain::<char, _, _>(digit().repeated().at_least(1));
        let number = mantissa
            .chain::<char, _, _>(exp.or_not().map(Option::unwrap_or_default))
            .collect::<String>()
            .map_with_span(|t: String, r| Expr::Num(t.parse().unwrap(), sp(r)));

        let ident_chars = filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_' || *c == '.');
        let product = just('$')
            .chain(filter(|c: &char| c.is_ascii_digit()).repeated().at_least(1))
            .chain(just('.'));
        let ident = product
            .or_not()
            .map(|p| p.unwrap_or_default())
            .chain::<char, _, _>(
                filter(|c: &char| c.is_ascii_alphabetic() || *c == '_')
                    .chain(ident_chars.repeated()),
            )
            .collect::<String>();

        let args = expr.clone().padded().separated_by(just(',')).delimited_by(
            filter(|c: &char| c.is_whitespace())
                .repeated()
                .ignore_then(just('(')),
            just(')'),
        );

        let atom = ident
            .then(args.or_not())
            .map_with_span(|(name, args), r| {
                let span = sp(r.clone());
                match args {
                    None => Ok(match name.as_str() {
                        "PI" => Expr::Const("PI", span),
                        "E" => Expr::Const("E", span),
                        "NaN" => Expr::Const("NaN", span),
                        _ => Expr::Ident(name, span),
                    }),
                    Some(args) => match FUNCS.iter().find(|f| f.0 == name) {
                        None => Err(Simple::custom(
                            r.start..r.start + name.len(),
                            format!("{UNKNOWN}{name}"),
                        )),
                        Some(&(_, _, n)) if args.len() != n => {
                            Err(Simple::custom(r, format!("{ARITY}{n}:{name}")))
                        }
                        Some(&(_, f, _)) => Ok(Expr::Call(f, args, span)),
                    },
                }
            })
            .try_map(|r, _| r)
            .or(number)
            .or(expr.clone().padded().delimited_by(just('('), just(')')))
            .padded()
            .boxed();

        // `^` is right-associative and binds tighter than unary prefix.
        let unary = recursive(|unary: Recursive<'a, char, Expr, Simple<char>>| {
            let pow = atom
                .clone()
                .then(just('^').padded().ignore_then(unary.clone()).or_not())
                .map(|(l, r)| match r {
                    None => l,
                    Some(r) => {
                        let s = join(&l, &r);
                        Expr::Binary(BinOp::Pow, Box::new(l), Box::new(r), s)
                    }
                });
            choice((just('-').to(UnOp::Neg), just('!').to(UnOp::Not)))
                .map_with_span(|o: UnOp, r: std::ops::Range<usize>| (o, r.start))
                .padded()
                .then(unary)
                .map(|((o, start), e)| {
                    let s = Span {
                        start,
                        end: e.span().end,
                    };
                    Expr::Unary(o, Box::new(e), s)
                })
                .or(pow)
        })
        .boxed();

        let mul = level(
            unary,
            choice((
                op("*", BinOp::Mul),
                op("/", BinOp::Div),
                op("%", BinOp::Rem),
            ))
            .boxed(),
        );
        let add = level(
            mul,
            choice((op("+", BinOp::Add), op("-", BinOp::Sub))).boxed(),
        );
        let rel = level(
            add,
            choice((
                op("<=", BinOp::Le),
                op(">=", BinOp::Ge),
                op("<", BinOp::Lt),
                op(">", BinOp::Gt),
            ))
            .boxed(),
        );
        let eq = level(
            rel,
            choice((op("==", BinOp::Eq), op("!=", BinOp::Ne))).boxed(),
        );
        let and = level(eq, op("&&", BinOp::And));
        let or = level(and, op("||", BinOp::Or));

        or.clone()
            .then(
                just('?')
                    .padded()
                    .ignore_then(expr.clone())
                    .then_ignore(just(':').padded())
                    .then(expr)
                    .or_not(),
            )
            .map(|(c, t)| match t {
                None => c,
                Some((a, b)) => {
                    let s = Span {
                        start: c.span().start,
                        end: b.span().end,
                    };
                    Expr::Ternary(Box::new(c), Box::new(a), Box::new(b), s)
                }
            })
    })
    .boxed()
}

const UNKNOWN: &str = "\0unknown:";
const ARITY: &str = "\0arity:";

pub fn parse(src: &str) -> Result<Expr, ExprError> {
    // Byte-indexed stream so every span is a byte offset into `src`.
    let stream = Stream::from_iter(
        src.len()..src.len(),
        src.char_indices().map(|(i, c)| (c, i..i + c.len_utf8())),
    );
    expr_parser()
        .padded()
        .then_ignore(end())
        .parse(stream)
        .map_err(|errs| {
            let e = &errs[0];
            let span = sp(e.span());
            match e.reason() {
                chumsky::error::SimpleReason::Custom(m) if m.starts_with(UNKNOWN) => {
                    ExprError::UnknownFunction {
                        name: m[UNKNOWN.len()..].to_string(),
                        span,
                    }
                }
                chumsky::error::SimpleReason::Custom(m) if m.starts_with(ARITY) => {
                    let (n, name) = m[ARITY.len()..].split_once(':').unwrap();
                    ExprError::Arity {
                        name: name.to_string(),
                        expected: n.parse().unwrap(),
                        span,
                    }
                }
                _ => ExprError::Parse {
                    msg: e.to_string(),
                    span,
                },
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinOp::*, Span};

    fn sp(start: usize, end: usize) -> Span {
        Span { start, end }
    }
    fn num(v: f64, s: usize, e: usize) -> Box<Expr> {
        Box::new(Expr::Num(v, sp(s, e)))
    }

    #[test]
    fn precedence() {
        // "1 + 2 * 3 ^ 2"
        let want = Expr::Binary(
            Add,
            num(1.0, 0, 1),
            Box::new(Expr::Binary(
                Mul,
                num(2.0, 4, 5),
                Box::new(Expr::Binary(
                    Pow,
                    num(3.0, 8, 9),
                    num(2.0, 12, 13),
                    sp(8, 13),
                )),
                sp(4, 13),
            )),
            sp(0, 13),
        );
        assert_eq!(parse("1 + 2 * 3 ^ 2").unwrap(), want);
    }

    #[test]
    fn unary_and_pow() {
        use crate::ast::UnOp;
        // -2^2 = -(2^2); 2^3^2 right-assoc
        let e = parse("-2^2").unwrap();
        assert!(matches!(&e, Expr::Unary(UnOp::Neg, b, _) if matches!(**b, Expr::Binary(Pow, ..))));
        let e = parse("2^3^2").unwrap();
        assert!(matches!(&e, Expr::Binary(Pow, _, r, _) if matches!(**r, Expr::Binary(Pow, ..))));
        assert!(parse("!!-1 + .5 * 1. + 1e-3").is_ok());
        assert_eq!(
            parse("$1.B4").unwrap(),
            Expr::Ident("$1.B4".into(), sp(0, 5))
        );
    }

    #[test]
    fn spans_are_byte_offsets_and_call_whitespace() {
        assert_eq!(
            parse("\u{3000}a\u{3000}").unwrap(),
            Expr::Ident("a".into(), sp(3, 4))
        );
        let strip = |e: Expr| match e {
            Expr::Call(f, a, _) => (f, a.len()),
            o => panic!("{o:?}"),
        };
        assert_eq!(
            strip(parse("sqrt (1)").unwrap()),
            strip(parse("sqrt(1)").unwrap())
        );
    }

    #[test]
    fn ternary_and_logic() {
        let e = parse("B4 > 0.3 && !nan(B8) ? B4 : NaN").unwrap();
        let names: Vec<_> = e.identifiers().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["B4", "B8", "B4"]);
        assert!(matches!(e, Expr::Ternary(..)));
    }

    #[test]
    fn parse_error_has_span() {
        match parse("B4 + ") {
            Err(ExprError::Parse { span, .. }) => {
                assert!(span == sp(4, 5) || span == sp(5, 5), "{span:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn arity_error() {
        match parse("sqrt(1, 2)") {
            Err(ExprError::Arity { expected, span, .. }) => {
                assert_eq!(expected, 1);
                assert_eq!(span, sp(0, 10));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_function() {
        match parse("1 + foo(2)") {
            Err(ExprError::UnknownFunction { name, span }) => {
                assert_eq!(name, "foo");
                assert_eq!(span, sp(4, 7));
            }
            other => panic!("{other:?}"),
        }
    }
}
