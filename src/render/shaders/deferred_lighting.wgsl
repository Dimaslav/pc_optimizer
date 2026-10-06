// Deferred lighting pass.
//
// Читает G-buffer (albedo+metallic, normal+depth, emissive+roughness),
// собирает освещение и пишет HDR в `hdr_view`.

const PI: f32 = 3.14159265359;

struct Camera {
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    view:           mat4x4<f32>,
    inv_view:       mat4x4<f32>,
    camera_pos:     vec4<f32>,
    near_far:       vec4<f32>,
    prev_view_proj: mat4x4<f32>,
    screen_size:    vec4<f32>,
};

struct Lights {
    cascade_vp:      array<mat4x4<f32>, 3>,
    cascade_splits:  vec4<f32>,
    ambient_color:   vec4<f32>,
    // x = dir_count, y = point_count, z = cube_shadow_count (0..4), w = unused
    counts:          vec4<u32>,
    light_view_proj: mat4x4<f32>,
    misc:            vec4<f32>,
    fog_params:      vec4<f32>,
    fog_color:       vec4<f32>,
    shadow_params:   vec4<f32>,
    dir_lights:      array<vec4<f32>, 8>,
    point_lights:    array<vec4<f32>, 32>,
    cube_shadow_pos: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var t_albedo: texture_2d<f32>;
@group(0) @binding(1) var t_normal: texture_2d<f32>;
@group(0) @binding(2) var t_emissive: texture_2d<f32>;
@group(0) @binding(3) var t_depth: texture_depth_2d;
@group(0) @binding(4) var s_lin: sampler;
@group(0) @binding(5) var<uniform> camera: Camera;

@group(1) @binding(0) var<uniform> lights: Lights;

@group(2) @binding(0) var t_csm: texture_depth_2d_array;
@group(2) @binding(1) var s_csm: sampler_comparison;
// ИЗМЕНЕНО (#7): cube → cube_array (до MAX_SHADOW_CUBES point-light теней).
@group(2) @binding(2) var t_point_shadow: texture_depth_cube_array;
@group(2) @binding(3) var s_point_shadow: sampler_comparison;
@group(2) @binding(4) var t_ssao: texture_2d<f32>;
@group(2) @binding(5) var s_ssao: sampler;
@group(2) @binding(6) var t_irradiance: texture_cube<f32>;
@group(2) @binding(7) var t_prefiltered: texture_cube<f32>;
@group(2) @binding(8) var t_brdf_lut: texture_2d<f32>;
@group(2) @binding(9) var s_env: sampler;
@group(2) @binding(10) var t_env: texture_cube<f32>;

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

fn reconstruct_world_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let world_h = camera.inv_view_proj * ndc;
    return world_h.xyz / max(world_h.w, 1e-6);
}

fn view_depth_of(world_pos: vec3<f32>) -> f32 {
    return -(camera.view * vec4<f32>(world_pos, 1.0)).z;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

// ============================================================
// Specular BRDF: single-scatter GGX
// ============================================================

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let n_dot_h2 = n_dot_h * n_dot_h;
    let denom = n_dot_h2 * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom + 1e-6);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k + 1e-6);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k + 1e-6);
    return gv * gl;
}

fn pbr_light(
    n: vec3<f32>, v: vec3<f32>, l: vec3<f32>,
    albedo: vec3<f32>, metallic: f32, roughness: f32, radiance: vec3<f32>,
) -> vec3<f32> {
    let n_dot_l = max(dot(n, l), 0.0);
    if (n_dot_l <= 0.0) { return vec3<f32>(0.0); }
    let n_dot_v = max(dot(n, v), 1e-4);
    let h = normalize(l + v);
    let n_dot_h = max(dot(n, h), 0.0);
    let h_dot_v = max(dot(h, v), 0.0);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let f = fresnel_schlick(h_dot_v, f0);
    let d = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);

    let specular = (d * g) * f / (4.0 * n_dot_v * n_dot_l + 1e-4);

    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * albedo / PI;

    return (diffuse + specular) * radiance * n_dot_l;
}

fn ibl_diffuse(n: vec3<f32>, albedo: vec3<f32>, metallic: f32) -> vec3<f32> {
    let irr = textureSampleLevel(t_irradiance, s_env, n, 0.0).rgb;
    return albedo * irr * (1.0 - metallic);
}

fn ibl_specular(n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, metallic: f32, roughness: f32) -> vec3<f32> {
    let r = reflect(-v, n);
    let mip = roughness * 4.0;
    let prefiltered = textureSampleLevel(t_prefiltered, s_env, r, mip).rgb;
    let n_dot_v = max(dot(n, v), 0.0);
    let brdf = textureSampleLevel(t_brdf_lut, s_env, vec2<f32>(n_dot_v, roughness), 0.0).rg;
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let scale = f0 * brdf.x + brdf.y;
    return prefiltered * scale;
}

// ============================================================
// CSM shadows с PCSS (Percentage-Closer Soft Shadows)
// ============================================================

const POISSON_16 = array<vec2<f32>, 16>(
    vec2<f32>(-0.94201624, -0.39906216),
    vec2<f32>( 0.94558609, -0.76890725),
    vec2<f32>(-0.09418410, -0.92938870),
    vec2<f32>( 0.34495938,  0.29387760),
    vec2<f32>(-0.91588581,  0.45771432),
    vec2<f32>(-0.81544232, -0.87912464),
    vec2<f32>(-0.38277543,  0.27676845),
    vec2<f32>( 0.97484398,  0.75648379),
    vec2<f32>( 0.44323325, -0.97511554),
    vec2<f32>( 0.53742981, -0.47373420),
    vec2<f32>(-0.26496911, -0.41893023),
    vec2<f32>( 0.79197514,  0.19090188),
    vec2<f32>(-0.24188840,  0.99706507),
    vec2<f32>(-0.81409955,  0.91437590),
    vec2<f32>( 0.19984126,  0.78641367),
    vec2<f32>( 0.14383161, -0.14100790),
);

const CSM_SIZE: i32 = 2048;
const CSM_SIZE_F: f32 = 2048.0;

fn csm_uv_and_depth(
    world_pos: vec3<f32>,
    cascade: i32,
) -> vec3<f32> {
    let light_clip = lights.cascade_vp[cascade] * vec4<f32>(world_pos, 1.0);
    if (light_clip.w < 1e-6) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    let ndc = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    if (ndc.z < 0.0 || ndc.z > 1.0) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    return vec3<f32>(uv.x, uv.y, ndc.z);
}

fn pcss_blocker_search(
    uv: vec2<f32>,
    frag_depth: f32,
    cascade: i32,
    texel: f32,
    search_radius: f32,
) -> vec2<f32> {
    var sum: f32 = 0.0;
    var count: f32 = 0.0;

    let step = texel * search_radius;

    for (var i: u32 = 0u; i < 16u; i = i + 1u) {
        let offset = POISSON_16[i] * step;
        let sample_uv = uv + offset;

        let pixel = vec2<i32>(sample_uv * CSM_SIZE_F);

        if (pixel.x < 0 || pixel.y < 0 || pixel.x >= CSM_SIZE || pixel.y >= CSM_SIZE) {
            continue;
        }

        let sample_depth = textureLoad(t_csm, pixel, cascade, 0);

        if (sample_depth < frag_depth) {
            sum += sample_depth;
            count += 1.0;
        }
    }

    if (count < 0.5) {
        return vec2<f32>(0.0, -1.0);
    }
    return vec2<f32>(sum / count, count);
}

fn pcss_filter(
    uv: vec2<f32>,
    frag_depth: f32,
    cascade: i32,
    texel: f32,
    filter_radius: f32,
    bias: f32,
) -> f32 {
    var shadow: f32 = 0.0;
    let step = texel * filter_radius;

    for (var i: u32 = 0u; i < 16u; i = i + 1u) {
        let offset = POISSON_16[i] * step;
        shadow += textureSampleCompareLevel(
            t_csm, s_csm,
            uv + offset,
            cascade,
            frag_depth - bias,
        );
    }
    return shadow / 16.0;
}

fn sample_csm_pcss(
    world_pos: vec3<f32>,
    cascade: i32,
    bias: f32,
) -> f32 {
    let uvz = csm_uv_and_depth(world_pos, cascade);
    if (uvz.z < 0.0) { return 1.0; }

    let uv = uvz.xy;
    let frag_depth = uvz.z;

    let texel: f32 = 1.0 / CSM_SIZE_F;

    let search_radius: f32 = 6.0;
    let filter_min: f32 = 1.0;
    let filter_max: f32 = 12.0;

    let blocker = pcss_blocker_search(uv, frag_depth, cascade, texel, search_radius);
    if (blocker.y < 0.0) {
        return 1.0;
    }
    let avg_blocker_depth = blocker.x;

    let penumbra = max(frag_depth - avg_blocker_depth, 0.0) / max(avg_blocker_depth, 1e-4);
    let filter_radius = clamp(
        filter_min + (filter_max - filter_min) * penumbra,
        filter_min,
        filter_max,
    );

    return pcss_filter(uv, frag_depth, cascade, texel, filter_radius, bias);
}

fn compute_csm_shadow(world_pos: vec3<f32>, view_depth: f32, n: vec3<f32>, l: vec3<f32>) -> f32 {
    let s = lights.shadow_params;
    let base_bias = s.x;
    let normal_bias = s.y;

    let ndl = clamp(dot(n, l), 0.0, 1.0);
    let slope = 1.0 - ndl;
    let bias = base_bias * (1.0 + normal_bias * slope);

    var cascade = 0;
    if (view_depth > lights.cascade_splits.x) { cascade = 1; }
    if (view_depth > lights.cascade_splits.y) { cascade = 2; }

    var shadow = sample_csm_pcss(world_pos, cascade, bias);

    if (cascade < 2) {
        var split_end = lights.cascade_splits.y;
        var split_prev = lights.cascade_splits.x;
        if (cascade == 0) {
            split_end = lights.cascade_splits.x;
            split_prev = 0.0;
        }
        let range = split_end - split_prev;
        let blend_zone = range * 0.15;
        let dist_to_edge = split_end - view_depth;

        if (dist_to_edge < blend_zone && dist_to_edge > 0.0) {
            let t = dist_to_edge / blend_zone;
            let shadow_next = sample_csm_pcss(world_pos, cascade + 1, bias);
            shadow = mix(shadow_next, shadow, t);
        }
    }

    let fade_start = s.z;
    let fade_end = s.w;
    if (view_depth > fade_start) {
        let fade = clamp((view_depth - fade_start) / max(fade_end - fade_start, 1e-4), 0.0, 1.0);
        shadow = mix(shadow, 1.0, fade);
    }

    return shadow;
}

/// ИЗМЕНЕНО (#7): принимает `cube_idx` — индекс point-light в массиве
/// теней. Раньше всегда читалось из нулевого куба, из-за чего теней
/// от point light'ов, кроме первого, не было.
///
/// `array_index` в `textureSampleCompare` для `texture_depth_cube_array`
/// — это индекс куба, не грань. Внутри — та же PCF 8 направлений.
fn compute_point_shadow(world_pos: vec3<f32>, cube_idx: u32) -> f32 {
    let pos = lights.cube_shadow_pos[cube_idx].xyz;
    let far = lights.cube_shadow_pos[cube_idx].w;
    let to_frag = world_pos - pos;
    let dist = length(to_frag);
    if (dist > far) { return 1.0; }

    let dir = to_frag / max(dist, 0.0001);
    let bias = lights.shadow_params.x * 3.0;
    let depth = dist / far - bias;

    // PCF по 8 направлениям вокруг dir.
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(dir.y) > 0.99);
    let t1 = normalize(cross(up, dir));
    let t2 = cross(dir, t1);
    let s = far * 2.0 / 1024.0 * 1.5;

    var shadow = 0.0;
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir + (t1 *  0.707 + t2 *  0.707) * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir + (t1 * -0.707 + t2 *  0.707) * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir + (t1 *  0.707 + t2 * -0.707) * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir + (t1 * -0.707 + t2 * -0.707) * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir +  t1 * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir -  t1 * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir +  t2 * s), cube_idx, depth);
    shadow += textureSampleCompare(t_point_shadow, s_point_shadow, normalize(dir -  t2 * s), cube_idx, depth);
    return shadow / 8.0;
}

// ============================================================
// fs_main
// ============================================================

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let depth = textureSample(t_depth, s_lin, in.uv);

    if (depth >= 0.9999) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let g_albedo   = textureSample(t_albedo,   s_lin, in.uv);
    let g_normal_d = textureSample(t_normal,   s_lin, in.uv);
    let g_emissive = textureSample(t_emissive, s_lin, in.uv);

    let albedo    = g_albedo.rgb;
    let metallic  = clamp(g_albedo.a, 0.0, 1.0);
    let roughness = clamp(g_emissive.a, 0.04, 1.0);
    let emissive  = g_emissive.rgb;
    let n         = normalize(g_normal_d.xyz);

    let world_pos = reconstruct_world_pos(in.uv, depth);
    let v = normalize(camera.camera_pos.xyz - world_pos);
    let view_depth = view_depth_of(world_pos);

    let ao = textureSample(t_ssao, s_ssao, in.uv).r;

    let ibl_strength = lights.misc.x;
    let ibl_diff = ibl_diffuse(n, albedo, metallic) * ao * ibl_strength;
    let ibl_spec = ibl_specular(n, v, albedo, metallic, roughness) * ao * ibl_strength;

    var direct = vec3<f32>(0.0);

    // Directional lights.
    let dir_count = lights.counts.x;
    for (var i: u32 = 0u; i < dir_count; i = i + 1u) {
        let dir_p = lights.dir_lights[i * 2u];
        let col_p = lights.dir_lights[i * 2u + 1u];
        let l = normalize(dir_p.xyz);
        let radiance = col_p.rgb * dir_p.w;
        let shadow = compute_csm_shadow(world_pos, view_depth, n, l);
        direct = direct + pbr_light(n, v, l, albedo, metallic, roughness, radiance * shadow);
    }

    // Point lights.
    let pt_count = lights.counts.y;
    // ИЗМЕНЕНО (#7): `counts.z` = число cube shadow maps (0..MAX_SHADOW_CUBES).
    let cube_shadow_count = lights.counts.z;

    for (var i: u32 = 0u; i < pt_count; i = i + 1u) {
        let pos_p = lights.point_lights[i * 2u];
        let col_p = lights.point_lights[i * 2u + 1u];
        let to_l = pos_p.xyz - world_pos;
        let dist = length(to_l);
        let range = pos_p.w;
        if (dist > range) { continue; }
        let l = to_l / max(dist, 1e-4);
        let atten = clamp(1.0 - dist / range, 0.0, 1.0);
        let atten2 = atten * atten;
        let radiance = col_p.rgb * col_p.a * atten2;

        // Тень — только для тех point light'ов, для которых запечена
        // cube shadow map (первые `cube_shadow_count` в массиве).
        var shadow = 1.0;
        if (i < cube_shadow_count) {
            shadow = compute_point_shadow(world_pos, i);
        }
        direct = direct + pbr_light(n, v, l, albedo, metallic, roughness, radiance * shadow);
    }

    var color = ibl_diff + ibl_spec + direct + emissive;

    // Height fog.
    let fog_density = lights.fog_params.x;
    if (fog_density > 0.0) {
        let dist = length(world_pos - camera.camera_pos.xyz);
        let h = max(0.0, world_pos.y - lights.fog_params.y);
        let h_factor = exp(-h * lights.fog_params.z);
        let fog_amount = clamp(1.0 - exp(-dist * fog_density * h_factor), 0.0, 1.0);
        color = mix(color, lights.fog_color.rgb, fog_amount);
    }

    return vec4<f32>(color, 1.0);
}