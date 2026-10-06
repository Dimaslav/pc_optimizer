struct Params {
    // xy = 1 / src_size (texel шаг источника), zw = 1 / dst_size
    texel: vec4<f32>,
    // x = threshold, y = soft_knee, z = radius (для upsample), w = unused
    params: vec4<f32>,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_lin: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VOut;
    out.clip = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ============================================================
// Prefilter: soft-knee bright pass
// ============================================================
// Логарифмически плавный вход над threshold: если L чуть-чуть выше
// threshold, вклад плавно нарастает от 0; если намного — почти 1.
// knee определяет ширину «мягкой зоны».
@fragment
fn fs_prefilter(in: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_src, s_lin, in.uv).rgb;
    let threshold = params.params.x;
    let knee = max(params.params.y, 1e-4);

    let l = luminance(c);

    // Unity URP / PostProcessing v2: quadratic soft knee.
    var soft = l - threshold + knee;
    soft = clamp(soft, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-6);
    let weight = max(soft, l - threshold) / max(l, 1e-6);

    return vec4<f32>(c * weight, 1.0);
}

// ============================================================
// Downsample: 13-tap (Jimenez / COD Advanced Warfare)
// ============================================================
// Схема тапов в источнике (d = texel шаг):
//
//      [10]       [11]
//            [0]
//      [2]   [1]   [3]
//            [4]
//      [12]       [9]
//            [6]
//      [5]   [7]   [8]
//
// Веса: центр-5 = 0.125, боковые-4 = 0.0625, диагональные-4 = 0.03125.
// Сумма = 0.125 + 4*0.125 + 4*0.0625 + 4*0.03125 = 1.0
@fragment
fn fs_downsample(in: VOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let d = params.texel.xy;

    // 5 центральных.
    var sum = textureSample(t_src, s_lin, uv).rgb * 0.125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( d.x,  d.y)).rgb * 0.125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-d.x,  d.y)).rgb * 0.125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( d.x, -d.y)).rgb * 0.125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-d.x, -d.y)).rgb * 0.125;

    // 4 боковых.
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( 2.0 * d.x, 0.0)).rgb * 0.0625;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-2.0 * d.x, 0.0)).rgb * 0.0625;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(0.0,  2.0 * d.y)).rgb * 0.0625;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(0.0, -2.0 * d.y)).rgb * 0.0625;

    // 4 диагональных.
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( 2.0 * d.x,  2.0 * d.y)).rgb * 0.03125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-2.0 * d.x,  2.0 * d.y)).rgb * 0.03125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( 2.0 * d.x, -2.0 * d.y)).rgb * 0.03125;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-2.0 * d.x, -2.0 * d.y)).rgb * 0.03125;

    return vec4<f32>(sum, 1.0);
}

// ============================================================
// Upsample: 3x3 tent filter, ADDITIVE blend
// ============================================================
// Pipeline использует blend: src=ONE, dst=ONE.
// Значит результат = bloom[i] (уже содержит downsample) + tent(bloom[i+1]).
@fragment
fn fs_upsample(in: VOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let d = params.texel.xy * max(params.params.z, 0.5); // radius

    var sum = vec3<f32>(0.0);

    // 3x3 tent (веса 1-2-1).
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-d.x, -d.y)).rgb * 1.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( 0.0, -d.y)).rgb * 2.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( d.x, -d.y)).rgb * 1.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-d.x,  0.0)).rgb * 2.0;
    sum += textureSample(t_src, s_lin, uv).rgb * 4.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( d.x,  0.0)).rgb * 2.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>(-d.x,  d.y)).rgb * 1.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( 0.0,  d.y)).rgb * 2.0;
    sum += textureSample(t_src, s_lin, uv + vec2<f32>( d.x,  d.y)).rgb * 1.0;

    return vec4<f32>(sum / 16.0, 1.0);
}