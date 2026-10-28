// Skin chrome: 9-slice frame patches, button sprites and the viewport fill.
//
// Each instance is a destination rectangle (window pixels) plus a source
// rectangle in the bound texture (texels). The fragment shader maps every
// destination pixel to a texel with integer math (`textureLoad`, no
// filtering), so the pixel art stays crisp at any integer scale. Tiled
// patches repeat their source rectangle; stretched ones scale it. Mirrors
// `hit::map_axis` on the CPU, which the hit-test mask uses.

struct Globals {
    viewport: vec2<f32>,
    mask_size: vec2<f32>,
    color_size: vec2<f32>,
    text_gamma: f32,
    srgb_target: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn out_color(c: vec3<f32>) -> vec3<f32> {
    if (g.srgb_target > 0.5) {
        return srgb_to_linear(c);
    }
    return c;
}

struct In {
    @builtin(vertex_index) vi: u32,
    @location(0) dst: vec4<f32>,
    @location(1) src: vec4<f32>,
    // x: stretch x, y: stretch y, z: texels-to-pixels factor k, w: kind (0 texture, 1 solid)
    @location(2) params: vec4<u32>,
    @location(3) color: vec4<f32>,
};

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) dst: vec4<f32>,
    @location(2) @interpolate(flat) src: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<u32>,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_chrome(in: In) -> Out {
    let c = vec2<f32>(f32(in.vi & 1u), f32((in.vi >> 1u) & 1u));
    let p = in.dst.xy + c * in.dst.zw;
    var o: Out;
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.local = c * in.dst.zw;
    o.dst = in.dst;
    o.src = in.src;
    o.params = in.params;
    o.color = in.color;
    return o;
}

fn axis(l: i32, dst_len: i32, src_o: i32, src_len: i32, k: i32, stretch: bool) -> i32 {
    let sl = max(src_len, 1);
    let ll = clamp(l, 0, max(dst_len - 1, 0));
    if (stretch) {
        return src_o + min(ll * sl / max(dst_len, 1), sl - 1);
    }
    return src_o + (ll / max(k, 1)) % sl;
}

@fragment
fn fs_chrome(in: Out) -> @location(0) vec4<f32> {
    if (in.params.w == 1u) {
        return vec4<f32>(out_color(in.color.rgb) * in.color.a, in.color.a);
    }
    let lx = i32(floor(in.local.x));
    let ly = i32(floor(in.local.y));
    let k = i32(in.params.z);
    let sx = axis(lx, i32(in.dst.z), i32(in.src.x), i32(in.src.z), k, in.params.x == 1u);
    let sy = axis(ly, i32(in.dst.w), i32(in.src.y), i32(in.src.w), k, in.params.y == 1u);
    let t = textureLoad(tex, vec2<i32>(sx, sy), 0);
    return vec4<f32>(out_color(t.rgb) * t.a, t.a);
}
