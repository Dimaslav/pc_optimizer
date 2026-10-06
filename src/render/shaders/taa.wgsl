// TAA resolve.
//
// Jitter в текущей версии всегда Vec2::ZERO (см. renderer/mod.rs).
// Тем не менее шейдер полностью поддерживает и jittered-конвенцию —
// формулы компенсации ниже работают при любом jitter_px.
//
// Конвенция (совпадает с gbuffer.wgsl / forward_transparent.wgsl):
//
//   - `t_current` содержит jittered-сцену. Пиксель (i, j) содержит
//     фрагмент F, чья unjittered-UV = ((i+0.5) − jitter_px) / screen.
//     При jitter = 0 это просто (i+0.5, j+0.5)/screen.
//
//   - `t_motion` в пикселе (i, j) описывает displacement
//     **unjittered**-позиции F между кадрами:
//       motion = prev_uv_F − uv_F_unjit_current
//     где uv_F_unjit_current = uv − jitter_uv.
//
//   - `t_history[x, y]` содержит resolved-цвет фрагмента, попавшего
//     в (x, y) при растеризации кадра N-1. Его unjittered-UV:
//       u_hist(x, y) = ((x+0.5) − prev_jitter_px) / screen
//
// Из этих трёх определений:
//   prev_uv_F = (uv − jitter_uv) + motion
//   history_uv = prev_uv_F + prev_jitter_uv
//
// При jitter = 0 формула сводится к классической:
//   history_uv = uv + motion

struct TaaParams {
    values: vec4<f32>,       // x = alpha, y = velocity_weight, z = sharpen, w = reset_flag
    screen: vec4<f32>,       // xy = (w_px, h_px), zw = jitter текущего кадра (пиксели)
    prev_jitter: vec4<f32>,  // xy = jitter предыдущего кадра (пиксели), zw = unused
};

@group(0) @binding(0) var t_current: texture_2d<f32>;
@group(0) @binding(1) var t_history: texture_2d<f32>;
@group(0) @binding(2) var t_motion:  texture_2d<f32>;
@group(0) @binding(3) var s_lin:     sampler;
@group(0) @binding(4) var<uniform> params: TaaParams;

struct VOut {
    @builtin(position) frag_coord: vec4<f32>,
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
    out.frag_coord = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    let y  = dot(c, vec3<f32>( 0.25,  0.5,  0.25));
    let co = dot(c, vec3<f32>( 0.5,   0.0, -0.5));
    let cg = dot(c, vec3<f32>(-0.25,  0.5, -0.25));
    return vec3<f32>(y, co, cg);
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let t = c.x - c.z;
    return vec3<f32>(
        t + c.y,
        c.x + c.z,
        t - c.y,
    );
}

fn clip_history(uv: vec2<f32>, texel: vec2<f32>, history: vec3<f32>) -> vec3<f32> {
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let o = vec2<f32>(f32(x), f32(y)) * texel;
            let s = rgb_to_ycocg(textureSample(t_current, s_lin, uv + o).rgb);
            m1 = m1 + s;
            m2 = m2 + s * s;
        }
    }
    m1 = m1 / 9.0;
    m2 = m2 / 9.0;
    let sigma = sqrt(max(m2 - m1 * m1, vec3<f32>(0.0)));
    let clip_min = m1 - 1.25 * sigma;
    let clip_max = m1 + 1.25 * sigma;

    let h_ycocg = rgb_to_ycocg(history);
    let clipped = clamp(h_ycocg, clip_min, clip_max);
    return ycocg_to_rgb(clipped);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let screen_px = params.screen.xy;
    let jitter_px = params.screen.zw;
    let prev_jitter_px = params.prev_jitter.xy;
    let texel = 1.0 / screen_px;

    let uv = in.uv;

    let pixel = vec2<i32>(i32(in.frag_coord.x), i32(in.frag_coord.y));
    let current = textureLoad(t_current, pixel, 0).rgb;

    if (params.values.w > 0.5) {
        return vec4<f32>(current, 1.0);
    }

    let motion = textureLoad(t_motion, pixel, 0).rg;
    let velocity_len = length(motion * screen_px);

    let jitter_uv = jitter_px / screen_px;
    let uv_unjit = uv - jitter_uv;
    let prev_uv = uv_unjit + motion;

    if (prev_uv.x < 0.0 || prev_uv.x > 1.0 || prev_uv.y < 0.0 || prev_uv.y > 1.0) {
        return vec4<f32>(current, 1.0);
    }

    let prev_jitter_uv = prev_jitter_px / screen_px;
    let history_uv = prev_uv + prev_jitter_uv;

    if (history_uv.x < 0.0 || history_uv.x > 1.0 || history_uv.y < 0.0 || history_uv.y > 1.0) {
        return vec4<f32>(current, 1.0);
    }

    let history_raw = textureSample(t_history, s_lin, history_uv).rgb;
    let history = clip_history(uv, texel, history_raw);

    let base_alpha = params.values.x;
    let velocity_weight = clamp(velocity_len * params.values.y, 0.0, 1.0);
    let alpha = mix(base_alpha, 1.0, velocity_weight);

    var blended = mix(history, current, alpha);

    let sharpen = params.values.z;
    if (sharpen > 0.001) {
        let n = textureSample(t_current, s_lin, uv + vec2<f32>( 0.0, -texel.y)).rgb;
        let s = textureSample(t_current, s_lin, uv + vec2<f32>( 0.0,  texel.y)).rgb;
        let e = textureSample(t_current, s_lin, uv + vec2<f32>( texel.x, 0.0)).rgb;
        let w = textureSample(t_current, s_lin, uv + vec2<f32>(-texel.x, 0.0)).rgb;
        let blur = (n + s + e + w) * 0.25;
        blended = blended + (blended - blur) * sharpen;
    }

    return vec4<f32>(max(blended, vec3<f32>(0.0)), 1.0);
}