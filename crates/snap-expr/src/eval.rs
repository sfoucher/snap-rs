use std::f32::consts::{E, PI};

use ndarray::Array2;
use snap_core::{GeoCoding, Tile};

use crate::{BinOp, Expr, ExprError, Func, UnOp};

/// One compiled node: (per-pixel values, X, Y) -> value. Values are the referenced
/// bands in `Compiled.bands` order, followed by LAT, LON when the expression uses them.
type Node = Box<dyn Fn(&[f32], f32, f32) -> f32 + Send + Sync>;

pub struct PixelCtx<'a> {
    pub origin_x: u32,
    pub origin_y: u32,
    pub level: u8,
    pub geocoding: Option<&'a dyn GeoCoding>,
}

pub struct Compiled {
    pub bands: Vec<String>,
    pub is_boolean: bool,
    uses_geo: bool,
    root: Node,
}

const PSEUDO: [&str; 4] = ["X", "Y", "LAT", "LON"];

pub fn compile(expr: &Expr, known_bands: &[String]) -> Result<Compiled, ExprError> {
    let mut bands: Vec<String> = Vec::new();
    let mut uses_geo = false;
    for (name, span) in expr.identifiers() {
        if known_bands.contains(&name) {
            if !bands.contains(&name) {
                bands.push(name);
            }
        } else if PSEUDO.contains(&name.as_str()) {
            uses_geo |= name == "LAT" || name == "LON";
        } else {
            return Err(ExprError::UnknownBand { name, span });
        }
    }
    let root = build(expr, &bands);
    Ok(Compiled {
        is_boolean: is_boolean(expr),
        bands,
        uses_geo,
        root,
    })
}

fn truthy(v: f32) -> bool {
    v != 0.0 && !v.is_nan()
}

fn bool_f(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn is_boolean(e: &Expr) -> bool {
    use BinOp::*;
    match e {
        Expr::Binary(op, ..) => matches!(op, Lt | Le | Gt | Ge | Eq | Ne | And | Or),
        Expr::Unary(UnOp::Not, ..) => true,
        Expr::Call(Func::Nan | Func::Inf, ..) => true,
        Expr::Ternary(_, a, b, _) => is_boolean(a) && is_boolean(b),
        _ => false,
    }
}

// Java Math.min/max semantics: NaN propagates (Rust's f32::min/max would drop it).
fn nan_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.min(b)
    }
}

fn nan_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else {
        a.max(b)
    }
}

fn build(e: &Expr, bands: &[String]) -> Node {
    let n = bands.len();
    match e {
        Expr::Num(v, _) => {
            let v = *v as f32;
            Box::new(move |_, _, _| v)
        }
        Expr::Const(c, _) => {
            let v = match *c {
                "PI" => PI,
                "E" => E,
                _ => f32::NAN,
            };
            Box::new(move |_, _, _| v)
        }
        Expr::Ident(name, _) => match bands.iter().position(|b| b == name) {
            Some(i) => Box::new(move |v, _, _| v[i]),
            None => match name.as_str() {
                "X" => Box::new(|_, x, _| x),
                "Y" => Box::new(|_, _, y| y),
                "LAT" => Box::new(move |v, _, _| v[n]),
                _ => Box::new(move |v, _, _| v[n + 1]), // LON; compile rejected anything else
            },
        },
        Expr::Unary(op, a, _) => {
            let a = build(a, bands);
            match op {
                UnOp::Neg => Box::new(move |v, x, y| -a(v, x, y)),
                UnOp::Not => Box::new(move |v, x, y| bool_f(!truthy(a(v, x, y)))),
            }
        }
        Expr::Binary(op, a, b, _) => {
            let (a, b) = (build(a, bands), build(b, bands));
            let f: fn(f32, f32) -> f32 = match op {
                BinOp::Add => |p, q| p + q,
                BinOp::Sub => |p, q| p - q,
                BinOp::Mul => |p, q| p * q,
                BinOp::Div => |p, q| p / q,
                BinOp::Rem => |p, q| p % q,
                BinOp::Pow => f32::powf,
                BinOp::Lt => |p, q| bool_f(p < q),
                BinOp::Le => |p, q| bool_f(p <= q),
                BinOp::Gt => |p, q| bool_f(p > q),
                BinOp::Ge => |p, q| bool_f(p >= q),
                BinOp::Eq => |p, q| bool_f(p == q),
                BinOp::Ne => |p, q| bool_f(p != q),
                BinOp::And => |p, q| bool_f(truthy(p) && truthy(q)),
                BinOp::Or => |p, q| bool_f(truthy(p) || truthy(q)),
            };
            Box::new(move |v, x, y| f(a(v, x, y), b(v, x, y)))
        }
        Expr::Ternary(c, a, b, _) => {
            let (c, a, b) = (build(c, bands), build(a, bands), build(b, bands));
            Box::new(move |v, x, y| {
                if truthy(c(v, x, y)) {
                    a(v, x, y)
                } else {
                    b(v, x, y)
                }
            })
        }
        // The parser guarantees each function's arity.
        Expr::Call(func, args, _) => {
            let a = build(&args[0], bands);
            let f2: Option<fn(f32, f32) -> f32> = match func {
                Func::Min => Some(nan_min),
                Func::Max => Some(nan_max),
                Func::Pow => Some(f32::powf),
                _ => None,
            };
            if let Some(f) = f2 {
                let b = build(&args[1], bands);
                return Box::new(move |v, x, y| f(a(v, x, y), b(v, x, y)));
            }
            let f: fn(f32) -> f32 = match func {
                Func::Sqrt => f32::sqrt,
                Func::Exp => f32::exp,
                Func::Log => f32::ln,
                Func::Log10 => f32::log10,
                Func::Abs => f32::abs,
                Func::Sin => f32::sin,
                Func::Cos => f32::cos,
                Func::Tan => f32::tan,
                Func::Nan => |p| bool_f(p.is_nan()),
                Func::Inf => |p| bool_f(p.is_infinite()),
                Func::Floor => f32::floor,
                Func::Ceil => f32::ceil,
                Func::Round => f32::round,
                Func::Min | Func::Max | Func::Pow => unreachable!(),
            };
            Box::new(move |v, x, y| f(a(v, x, y)))
        }
    }
}

impl Compiled {
    /// inputs in the same order as `self.bands`; all tiles same shape. Returns F32, or U8 (0/1) if is_boolean.
    /// With no referenced bands there is no shape to take, so a 1x1 tile is evaluated.
    pub fn eval_tile(&self, inputs: &[&Tile], ctx: &PixelCtx) -> Tile {
        let shape = inputs.first().map_or((1, 1), |t| t.shape());
        let arrays: Vec<Array2<f32>> = inputs.iter().map(|t| t.to_f32()).collect();
        self.eval_arrays(&arrays, shape, ctx)
    }

    /// `shape` is (rows, cols); `inputs` are already-converted band values.
    pub(crate) fn eval_arrays(
        &self,
        inputs: &[Array2<f32>],
        shape: (usize, usize),
        ctx: &PixelCtx,
    ) -> Tile {
        let n = inputs.len();
        let scale = (1u64 << ctx.level) as f64;
        let mut vals = vec![0f32; n + if self.uses_geo { 2 } else { 0 }];
        let out = Array2::from_shape_fn(shape, |(r, c)| {
            for (v, a) in vals.iter_mut().zip(inputs) {
                *v = a[(r, c)];
            }
            let x = (ctx.origin_x as f64 + c as f64 + 0.5) * scale;
            let y = (ctx.origin_y as f64 + r as f64 + 0.5) * scale;
            if self.uses_geo {
                let g = ctx.geocoding.and_then(|g| g.pixel_to_geo(x, y));
                vals[n] = g.map_or(f32::NAN, |g| g.lat as f32);
                vals[n + 1] = g.map_or(f32::NAN, |g| g.lon as f32);
            }
            (self.root)(&vals, x as f32, y as f32)
        });
        if self.is_boolean {
            Tile::U8(out.mapv(|v| truthy(v) as u8))
        } else {
            Tile::F32(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse, Span};
    use ndarray::arr2;

    const CTX: PixelCtx = PixelCtx {
        origin_x: 0,
        origin_y: 0,
        level: 0,
        geocoding: None,
    };

    fn bands() -> Vec<String> {
        vec!["B4".to_string(), "B8".to_string()]
    }

    /// Evaluates `e` on 1x1 tiles with B4 = b4, B8 = b8 (inputs ordered as `Compiled.bands`).
    fn eval1(e: &str, b4: f32, b8: f32) -> Tile {
        let c = compile(&parse(e).unwrap(), &bands()).unwrap();
        let t4 = Tile::F32(arr2(&[[b4]]));
        let t8 = Tile::F32(arr2(&[[b8]]));
        let inputs: Vec<&Tile> = c
            .bands
            .iter()
            .map(|n| if n == "B4" { &t4 } else { &t8 })
            .collect();
        c.eval_tile(&inputs, &CTX)
    }

    #[test]
    fn snap_reference_table() {
        // (expr, B4, B8, expected) — values checked in SNAP Band Maths on constants
        for (e, b4, b8, want) in [
            ("(B8-B4)/(B8+B4)", 0.1, 0.5, 0.6666667),
            ("B4 > 0.3 ? 1 : 0", 0.5, 0.0, 1.0),
            ("max(B4,B8)", 0.1, 0.5, 0.5),
            ("log10(B8)", 0.1, 100.0, 2.0),
            ("B4 % 0.3", 0.7, 0.0, 0.1),
        ] {
            let Tile::F32(a) = eval1(e, b4, b8) else {
                panic!("{e}: expected F32 tile")
            };
            let got: f32 = a[(0, 0)];
            let ok = (got - want).abs() <= 1e-6 || (got - want).abs() <= 1e-5 * want.abs();
            assert!(ok, "{e}: got {got}, want {want}");
        }
    }

    #[test]
    fn boolean_yields_u8() {
        match eval1("B4 > 0.3 && !nan(B8)", 0.5, f32::NAN) {
            Tile::U8(a) => assert_eq!(a[(0, 0)], 0),
            t => panic!("expected U8, got {:?}", t.data_type()),
        }
        match eval1("B4 > 0.3 && !nan(B8)", 0.5, 1.0) {
            Tile::U8(a) => assert_eq!(a[(0, 0)], 1),
            t => panic!("expected U8, got {:?}", t.data_type()),
        }
    }

    #[test]
    fn unknown_band_is_bind_error() {
        let r = compile(&parse("B99").unwrap(), &["B4".to_string()]);
        assert_eq!(
            r.err(),
            Some(ExprError::UnknownBand {
                name: "B99".into(),
                span: Span { start: 0, end: 3 }
            })
        );
    }

    #[test]
    fn bands_deduplicated_in_first_appearance_order() {
        let c = compile(&parse("B8 + B4 * B8 + X").unwrap(), &bands()).unwrap();
        assert_eq!(c.bands, vec!["B8".to_string(), "B4".to_string()]);
    }

    #[test]
    fn x_y_are_level0_pixel_centres() {
        let c = compile(&parse("X + 1000 * Y").unwrap(), &bands()).unwrap();
        let ctx = PixelCtx {
            origin_x: 512,
            origin_y: 0,
            level: 1,
            geocoding: None,
        };
        let Tile::F32(a) = c.eval_tile(&[], &ctx) else {
            panic!()
        };
        // X = (512 + 0 + 0.5) * 2, Y = (0 + 0 + 0.5) * 2
        assert_eq!(a[(0, 0)], 1025.0 + 1000.0);
    }
}
