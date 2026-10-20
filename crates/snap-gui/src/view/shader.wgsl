struct Params {
    min: vec3<f32>,
    max: vec3<f32>,
    gamma: vec3<f32>,
    mode: u32, // 0 = single band through the LUT, 1 = RGB, 2 = mask overlay
    color: vec4<f32>, // mode 2: rgb colour, a = opacity
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var lut: texture_2d<f32>;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var t1: texture_2d<f32>;
@group(1) @binding(2) var t2: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VOut {
    var o: VOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.uv = uv;
    return o;
}

// R32Float is not filterable, so fetch the nearest texel directly.
fn texel(t: texture_2d<f32>, uv: vec2<f32>) -> f32 {
    let d = vec2<i32>(textureDimensions(t));
    let c = clamp(vec2<i32>(floor(uv * vec2<f32>(d))), vec2<i32>(0), d - 1);
    return textureLoad(t, c, 0).r;
}

// `x != x` may be folded away by shader compilers; test the bits instead.
fn is_nan(x: f32) -> bool {
    let b = bitcast<u32>(x);
    return (b & 0x7f800000u) == 0x7f800000u && (b & 0x007fffffu) != 0u;
}

fn chan(x: f32, i: u32) -> f32 {
    // Dragged min/max can cross: treat an empty window as 1 unit wide.
    let w = p.max[i] - p.min[i];
    let v = clamp((x - p.min[i]) / select(1.0, w, w > 1e-20), 0.0, 1.0);
    return pow(v, 1.0 / max(p.gamma[i], 1e-3));
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let r = texel(t0, in.uv);
    if p.mode == 2u {
        // Premultiplied; mask tiles are R8Unorm, so a set pixel reads 1.0.
        return select(vec4<f32>(0.0), vec4<f32>(p.color.rgb * p.color.a, p.color.a), r > 0.5);
    }
    if p.mode == 0u {
        if is_nan(r) {
            return vec4<f32>(0.0);
        }
        let i = i32(round(chan(r, 0u) * 255.0));
        return vec4<f32>(textureLoad(lut, vec2<i32>(i, 0), 0).rgb, 1.0);
    }
    let g = texel(t1, in.uv);
    let b = texel(t2, in.uv);
    if is_nan(r) || is_nan(g) || is_nan(b) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(chan(r, 0u), chan(g, 1u), chan(b, 2u), 1.0);
}
