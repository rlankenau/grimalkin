// Kitty graphics images: textured quads, plus a tiny blit used to build mip
// chains on the GPU.
//
// Image textures are Rgba8UnormSrgb, so sampling returns linear color with
// straight alpha. The surface is normally a non-sRGB format and the rest of
// the renderer blends in encoded space, so we re-encode here unless the
// surface itself is sRGB (`g.srgb_target`).

struct Globals {
    viewport: vec2<f32>,
    mask_size: vec2<f32>,
    color_size: vec2<f32>,
    text_gamma: f32,
    srgb_target: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var img_tex: texture_2d<f32>;
@group(0) @binding(2) var img_samp: sampler;

struct ImgIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
};

struct ImgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_image(in: ImgIn) -> ImgOut {
    let c = vec2<f32>(f32(in.vi & 1u), f32((in.vi >> 1u) & 1u));
    let p = in.pos + c * in.size;
    var o: ImgOut;
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.uv = mix(in.uv0, in.uv1, c);
    return o;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_image(in: ImgOut) -> @location(0) vec4<f32> {
    let t = textureSample(img_tex, img_samp, in.uv);
    var rgb = t.rgb;
    if (g.srgb_target < 0.5) {
        rgb = linear_to_srgb(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return vec4<f32>(rgb * t.a, t.a);
}

// ------------------------------------------------------------ mip blit ----
// 2x2 box filter that weights colors by alpha, so transparent texels (often
// black) do not darken the edges of shrunken images. Reads linear values from
// the sRGB source level; the sRGB render target re-encodes the result.

@group(0) @binding(0) var src_tex: texture_2d<f32>;

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Full-screen triangle.
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}

@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(src_tex));
    let base = vec2<i32>(floor(pos.xy)) * 2;
    let hi = dims - vec2<i32>(1);
    let a = textureLoad(src_tex, min(base, hi), 0);
    let b = textureLoad(src_tex, min(base + vec2<i32>(1, 0), hi), 0);
    let c = textureLoad(src_tex, min(base + vec2<i32>(0, 1), hi), 0);
    let d = textureLoad(src_tex, min(base + vec2<i32>(1, 1), hi), 0);
    let wsum = a.a + b.a + c.a + d.a;
    var rgb = (a.rgb + b.rgb + c.rgb + d.rgb) * 0.25;
    if (wsum > 0.0) {
        rgb = (a.rgb * a.a + b.rgb * b.a + c.rgb * c.a + d.rgb * d.a) / wsum;
    }
    return vec4<f32>(rgb, wsum * 0.25);
}
