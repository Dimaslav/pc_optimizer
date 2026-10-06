// Tonemap + post-processing финал.
//
// AgX (Sobotka 2022, "Minimal AgX").
//
// ИСПРАВЛЕНО: в оригинальном шейдере была потеряна нормализация.
// Правильная формула:
//
//     x' = (log2(x) − AGX_MIN_EV) / (AGX_MAX_EV − AGX_MIN_EV)
//
// В старой версии было:
//
//     x' = log2(x) + ( −AGX_MIN_EV / (AGX_MAX_EV − AGX_MIN_EV) )
//
// Также убран post-AgX contrast-boost: он жёстко клипал значения
// выше 0.9 в 1.0, обрезая highlight rolloff — ради которого AgX
// и используется.

@group(0) @binding(0) var t_hdr: texture_2d<f32>;
@group(0) @binding(1) var t_bloom: texture_2d<f32>;
@group(0) @binding(2) var s_lin: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct Params {
    // x = bloom strength, y = exposure, z = time (для grain), w = unused
    values: vec4<f32>,
    // x = vignette_strength, y = grain_strength, z = chromatic_strength, w = unused
    effects: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
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
    var out: VertexOutput;
    out.clip = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

// ============================================================
// AgX tonemap (Sobotka 2022, "Minimal AgX")
// ============================================================

const AGX_MAT_IN: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>( 0.842479062253094,  0.0423282422610123, 0.0423756549057051),
    vec3<f32>( 0.0784335999999992, 0.878468636469772,  0.0784336),
    vec3<f32>( 0.0792237451477643, 0.0791661274605434, 0.879142973793104),
);

const AGX_MAT_OUT: mat3x3<f32> = mat3x3<f32>(
    vec3<f32>( 1.19687900512017,  -0.0528968517574562, -0.0529716355144438),
    vec3<f32>(-0.0980208811401368, 1.15190312990417,   -0.0980434501171241),
    vec3<f32>(-0.0990297440797205,-0.0989611768448433, 1.15107367264116),
);

const AGX_MIN_EV: f32 = -12.47393;
const AGX_MAX_EV: f32 = 4.026069;

fn agx_default_contrast_approx(x: vec3<f32>) -> vec3<f32> {
    // Полиномиальная аппроксимация sigmoid из AgX.
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2
         - 40.14 * x4 * x
         + 31.96 * x4
         - 6.868 * x2 * x
         + 0.4298 * x2
         + 0.1191 * x
         - 0.00232;
}

fn agx(x: vec3<f32>) -> vec3<f32> {
    var v = AGX_MAT_IN * x;

    // Правильная нормализация AgX.
    let log_v = log2(max(v, vec3<f32>(1e-10)));
    v = clamp(
        (log_v - vec3<f32>(AGX_MIN_EV)) / (AGX_MAX_EV - AGX_MIN_EV),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );

    // Sigmoid.
    v = agx_default_contrast_approx(v);

    // Обратно в linear.
    v = AGX_MAT_OUT * v;

    return v;
}

// ============================================================
// Hash / noise helpers
// ============================================================

fn hash13(p3_in: vec3<f32>) -> f32 {
    var p3 = fract(p3_in * 0.1031);
    p3 = p3 + dot(p3, p3.zyx + vec3<f32>(31.32));
    return fract((p3.x + p3.y) * p3.z);
}

// Простой 3D value noise для film grain.
fn film_grain(uv: vec2<f32>, t: f32) -> f32 {
    let p = vec3<f32>(uv * 1024.0, t);
    return hash13(p) * 2.0 - 1.0;
}

// ============================================================
// fs_main
// ============================================================

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = in.uv;

    // --- Хроматическая аберрация ---------------------------------
    let chromatic = params.effects.z;
    var hdr: vec3<f32>;
    if (chromatic > 0.0001) {
        let center = vec2<f32>(0.5, 0.5);
        let to_center = uv - center;
        let r2 = dot(to_center, to_center);
        let shift = chromatic * r2 * 0.004;
        let dir = normalize(to_center + vec2<f32>(1e-6, 1e-6));

        let r = textureSample(t_hdr, s_lin, uv + dir * shift).r;
        let g = textureSample(t_hdr, s_lin, uv).g;
        let b = textureSample(t_hdr, s_lin, uv - dir * shift).b;
        hdr = vec3<f32>(r, g, b);
    } else {
        hdr = textureSample(t_hdr, s_lin, uv).rgb;
    }

    // --- Bloom ---------------------------------------------------
    let bloom = textureSample(t_bloom, s_lin, uv).rgb;
    let strength = params.values.x;
    hdr = hdr + bloom * strength;

    // --- Exposure ------------------------------------------------
    hdr = hdr * params.values.y;

    // --- AgX tonemap --------------------------------------------
    // ИСПРАВЛЕНО: убран post-AgX contrast-boost. Он делал
    //   out = 0.5 + (out - 0.5) * 1.25
    // что для out = 0.9 давало 1.0, а для out = 0.95 → 1.0625 → clamp → 1.0.
    // Это убивало highlight rolloff AgX и превращало светлые части
    // сцены в чистый белый. AgX сам по себе имеет правильную кривую.
    var out_rgb = agx(max(hdr, vec3<f32>(0.0)));

    // --- Vignette ------------------------------------------------
    let vignette = params.effects.x;
    if (vignette > 0.0001) {
        let center = vec2<f32>(0.5, 0.5);
        let d = distance(uv, center) * 1.4142;
        let v = smoothstep(1.0, 0.3, d * (1.0 + vignette));
        out_rgb = out_rgb * mix(1.0, v, vignette);
    }

    // --- Film grain (в perceptual sRGB пространстве) ------------
    let grain = params.effects.y;
    if (grain > 0.0001) {
        let t = floor(params.values.z * 24.0);
        let g = film_grain(uv, t);
        let luma = dot(out_rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let grain_mod = mix(1.0, 0.4, luma);
        out_rgb = clamp(out_rgb + g * grain * 0.08 * grain_mod, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    return vec4<f32>(out_rgb, 1.0);
}