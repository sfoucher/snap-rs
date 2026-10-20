use crate::view::Palette;
use std::collections::HashMap;

/// Parses a SNAP `.cpd` (Java properties): `numPoints`, `colorK=r,g,b[,a]`, `sampleK`.
/// Unknown keys are ignored; samples are normalised to 0..1 by min/max sample.
pub fn parse_cpd(text: &str) -> Result<Palette, String> {
    let props: HashMap<&str, &str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect();
    let n: usize = props
        .get("numPoints")
        .ok_or("missing numPoints")?
        .parse()
        .map_err(|_| "invalid numPoints".to_string())?;
    if n < 2 {
        return Err(format!("numPoints must be at least 2, got {n}"));
    }
    let mut pts = Vec::with_capacity(n);
    for k in 0..n {
        let c = props
            .get(format!("color{k}").as_str())
            .ok_or(format!("missing color{k}"))?;
        let rgb: Vec<u8> = c
            .split(',')
            .map(|v| v.trim().parse::<u8>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("invalid color{k}"))?;
        if rgb.len() < 3 || rgb.len() > 4 {
            return Err(format!("invalid color{k}"));
        }
        let s: f32 = props
            .get(format!("sample{k}").as_str())
            .ok_or(format!("missing sample{k}"))?
            .parse()
            .map_err(|_| format!("invalid sample{k}"))?;
        if !s.is_finite() {
            return Err(format!("invalid sample{k}"));
        }
        pts.push((s, [rgb[0], rgb[1], rgb[2]]));
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (lo, hi) = (pts[0].0, pts[n - 1].0);
    let stops = pts
        .into_iter()
        .map(|(s, c)| (if hi > lo { (s - lo) / (hi - lo) } else { 0.0 }, c))
        .collect();
    Ok(Palette {
        name: "Custom".into(),
        stops,
    })
}

pub fn builtin_palettes() -> Vec<Palette> {
    let p = |name: &str, colors: &[[u8; 3]]| Palette {
        name: name.into(),
        stops: colors
            .iter()
            .enumerate()
            .map(|(i, &c)| (i as f32 / (colors.len() - 1) as f32, c))
            .collect(),
    };
    vec![
        Palette::default(),
        p(
            "Spectrum",
            &[
                [0, 0, 255],
                [0, 255, 255],
                [0, 255, 0],
                [255, 255, 0],
                [255, 0, 0],
            ],
        ),
        p(
            "Viridis",
            &[
                [68, 1, 84],
                [59, 82, 139],
                [33, 145, 140],
                [94, 201, 98],
                [253, 231, 37],
            ],
        ),
        p("Red", &[[0, 0, 0], [255, 0, 0]]),
        p("Green", &[[0, 0, 0], [0, 255, 0]]),
        p("Blue", &[[0, 0, 0], [0, 0, 255]]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpd_two_points() {
        let p =
            parse_cpd("numPoints=2\ncolor0=0,0,0\nsample0=10\ncolor1=255,255,255\nsample1=20\n")
                .unwrap();
        assert_eq!(p.stops, vec![(0.0, [0, 0, 0]), (1.0, [255, 255, 255])]);
    }

    #[test]
    fn parse_cpd_ignores_extras_and_sorts() {
        let p = parse_cpd(
            "# c\nautoDistribute=true\nnumPoints=3\ncolor0=1,2,3,255\nsample0=5\n\
             color1=9,9,9\nsample1=-5\ncolor2=7,7,7\nsample2=0\n",
        )
        .unwrap();
        assert_eq!(
            p.stops,
            vec![(0.0, [9, 9, 9]), (0.5, [7, 7, 7]), (1.0, [1, 2, 3])]
        );
    }

    #[test]
    fn parse_cpd_errors() {
        assert!(parse_cpd("color0=0,0,0").unwrap_err().contains("numPoints"));
        assert!(parse_cpd("numPoints=1\ncolor0=0,0,0\nsample0=1").is_err());
        assert!(
            parse_cpd("numPoints=2\ncolor0=0,0,0\nsample0=1\ncolor1=1,1,1")
                .unwrap_err()
                .contains("sample1")
        );
        assert!(parse_cpd("numPoints=2\ncolor0=x\nsample0=1\ncolor1=1,1,1\nsample1=2").is_err());
    }

    #[test]
    fn builtins_are_usable() {
        let all = builtin_palettes();
        assert_eq!(all[0].name, "Grey");
        assert!(all.iter().all(|p| p.stops.len() >= 2));
    }
}
