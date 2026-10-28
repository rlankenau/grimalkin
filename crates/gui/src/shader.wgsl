// Grimalkin cell renderer.
//
// All positions are in physical pixels (origin top-left). Colors arrive as
// sRGB-encoded bytes. The surface is a non-sRGB format, so we blend directly in
// the encoded space (like most terminals); `globals.srgb_target` converts to
// linear when only an sRGB surface format is available.

struct Globals {
    viewport: vec2<f32>,
    mask_size: vec2<f32>,
    color_size: vec2<f32>,
    // Exponent applied to glyph coverage (< 1 thickens text slightly).
    text_gamma: f32,
    srgb_target: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

fn corner(vi: u32) -> vec2<f32> {
    // Triangle strip: (0,0) (1,0) (0,1) (1,1)
    return vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
}

fn to_ndc(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
}

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

// ---------------------------------------------------------------- rects ----

struct RectIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: u32,
};

struct RectOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) @interpolate(flat) kind: u32,
};

@vertex
fn vs_rect(in: RectIn) -> RectOut {
    let c = corner(in.vi);
    var o: RectOut;
    o.clip = to_ndc(in.pos + c * in.size);
    o.color = in.color;
    o.local = c * in.size;
    o.size = in.size;
    o.kind = in.kind;
    return o;
}

@fragment
fn fs_rect(in: RectOut) -> @location(0) vec4<f32> {
    var a = in.color.a;
    if (in.kind == 1u) {
        // Curly underline: sine wave one cell wide inside a box of height 3t.
        let t = in.size.y / 3.0;
        let amp = (in.size.y - t) * 0.5;
        let phase = in.local.x / in.size.x * 6.2831853;
        let center = in.size.y * 0.5 + amp * sin(phase);
        let slope = amp * cos(phase) * 6.2831853 / in.size.x;
        let d = abs(in.local.y - center) / sqrt(1.0 + slope * slope);
        a = a * clamp(t * 0.5 + 0.5 - d, 0.0, 1.0);
    }
    return vec4<f32>(out_color(in.color.rgb) * a, a);
}

// --------------------------------------------------------------- glyphs ----

struct GlyphIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv: vec4<u32>,
    @location(3) color: vec4<f32>,
};

struct GlyphOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) tex: vec2<f32>,
};

fn glyph_vertex(in: GlyphIn, atlas_size: vec2<f32>) -> GlyphOut {
    let c = corner(in.vi);
    var o: GlyphOut;
    o.clip = to_ndc(in.pos + c * in.size);
    o.color = in.color;
    let uv = vec4<f32>(in.uv);
    o.tex = (uv.xy + c * uv.zw) / atlas_size;
    return o;
}

@vertex
fn vs_mask(in: GlyphIn) -> GlyphOut {
    return glyph_vertex(in, g.mask_size);
}

@vertex
fn vs_color(in: GlyphIn) -> GlyphOut {
    return glyph_vertex(in, g.color_size);
}

@fragment
fn fs_mask(in: GlyphOut) -> @location(0) vec4<f32> {
    let cov = textureSample(atlas_tex, atlas_samp, in.tex).r;
    let a = pow(cov, g.text_gamma) * in.color.a;
    return vec4<f32>(out_color(in.color.rgb) * a, a);
}

@fragment
fn fs_color(in: GlyphOut) -> @location(0) vec4<f32> {
    // Atlas holds premultiplied RGBA; color.a carries dimming/opacity.
    let t = textureSample(atlas_tex, atlas_samp, in.tex);
    var rgb = t.rgb;
    if (g.srgb_target > 0.5) {
        // Un-premultiply, convert, re-premultiply.
        let straight = select(vec3<f32>(0.0), rgb / t.a, t.a > 0.0);
        rgb = srgb_to_linear(straight) * t.a;
    }
    return vec4<f32>(rgb, t.a) * in.color.a;
}
