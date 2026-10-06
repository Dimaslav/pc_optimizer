struct Params {
    // xy = inverse size, z = strength, w = unused
    values: vec4<f32>,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
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

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let inv = params.values.xy;
    let strength = params.values.z;

    if (strength < 0.001) {
        return vec4<f32>(textureSample(t_src, s_src, in.uv).rgb, 1.0);
    }

    let rgbM  = textureSample(t_src, s_src, in.uv).rgb;
    let rgbNW = textureSample(t_src, s_src, in.uv + vec2<f32>(-1.0, -1.0) * inv).rgb;
    let rgbNE = textureSample(t_src, s_src, in.uv + vec2<f32>( 1.0, -1.0) * inv).rgb;
    let rgbSW = textureSample(t_src, s_src, in.uv + vec2<f32>(-1.0,  1.0) * inv).rgb;
    let rgbSE = textureSample(t_src, s_src, in.uv + vec2<f32>( 1.0,  1.0) * inv).rgb;

    let lM  = luma(rgbM);
    let lNW = luma(rgbNW);
    let lNE = luma(rgbNE);
    let lSW = luma(rgbSW);
    let lSE = luma(rgbSE);

    let lMin = min(lM, min(min(lNW, lNE), min(lSW, lSE)));
    let lMax = max(lM, max(max(lNW, lNE), max(lSW, lSE)));

    let range = lMax - lMin;
    if (range < 0.05 * max(lMax, 0.1)) {
        return vec4<f32>(rgbM, 1.0);
    }

    let dir_x = -((lNW + lNE) - (lSW + lSE));
    let dir_y =  ((lNW + lSW) - (lNE + lSE));
    let dir_reduce = max((lNW + lNE + lSW + lSE) * 0.25 * 0.5, 0.0001);
    let rcp_dir_min = 1.0 / (min(abs(dir_x), abs(dir_y)) + dir_reduce);

    var dir = vec2<f32>(
        clamp(dir_x * rcp_dir_min, -2.0, 2.0),
        clamp(dir_y * rcp_dir_min, -2.0, 2.0),
    );
    dir = vec2<f32>(clamp(dir.x, -1.5, 1.5), clamp(dir.y, -1.5, 1.5)) * inv;

    let rgbA = 0.5 * (
        textureSample(t_src, s_src, in.uv + dir * (1.0 / 3.0 - 0.5)).rgb +
        textureSample(t_src, s_src, in.uv + dir * (2.0 / 3.0 - 0.5)).rgb
    );
    let rgbB = rgbA * 0.5 + 0.25 * (
        textureSample(t_src, s_src, in.uv + dir * -0.5).rgb +
        textureSample(t_src, s_src, in.uv + dir *  0.5).rgb
    );

    let lB = luma(rgbB);
    let fxaa_color = select(rgbB, rgbA, lB < lMin || lB > lMax);

    return vec4<f32>(mix(rgbM, fxaa_color, strength), 1.0);
}