// Volumetric fog composite + SSR.
//
// Читает HDR + froxel-texture + G-buffer (depth, normal, albedo, emissive).
// Порядок:
//   1. SSR — screen-space reflections, добавляется к HDR.
//   2. Fog composite — накладывается поверх.
//   3. Результат идёт в hdr_fog_view, откуда его читает TAA.

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

struct Volumetric {
    grid: vec4<f32>,
    cam: vec4<f32>,
    params: vec4<f32>,
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var t_hdr:   texture_2d<f32>;
@group(0) @binding(1) var t_fog:   texture_3d<f32>;
@group(0) @binding(2) var s_lin:   sampler;
@group(0) @binding(3) var t_depth: texture_depth_2d;
@group(0) @binding(4) var<uniform> vol: Volumetric;

@group(0) @binding(5) var t_normal:   texture_2d<f32>;
@group(0) @binding(6) var t_albedo:   texture_2d<f32>;
@group(0) @binding(7) var t_emissive: texture_2d<f32>;

@group(0) @binding(8) var<uniform> camera: Camera;

const FROXEL_Z: f32 = 64.0;

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

fn load_depth(uv: vec2<f32>) -> f32 {
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return 1.0;
    }
    let dim = vec2<f32>(textureDimensions(t_depth));
    let pixel = vec2<i32>(uv * dim);
    let clamped = clamp(
        pixel,
        vec2<i32>(0, 0),
        vec2<i32>(dim) - vec2<i32>(1, 1),
    );
    return textureLoad(t_depth, clamped, 0);
}

fn reconstruct_world_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let world_h = camera.inv_view_proj * ndc;
    return world_h.xyz / max(world_h.w, 1e-6);
}

fn project_world_to_uv(world_pos: vec3<f32>) -> vec3<f32> {
    let clip = camera.view_proj * vec4<f32>(world_pos, 1.0);
    if (clip.w < 1e-6) { return vec3<f32>(0.0, 0.0, -1.0); }
    let ndc = clip.xyz / clip.w;
    if (ndc.z < 0.0 || ndc.z > 1.0) { return vec3<f32>(0.0, 0.0, -1.0); }
    return vec3<f32>(
        ndc.x * 0.5 + 0.5,
        1.0 - (ndc.y * 0.5 + 0.5),
        ndc.z,
    );
}

fn trace_ssr(
    world_pos: vec3<f32>,
    world_normal: vec3<f32>,
    view_dir: vec3<f32>,
    fresnel_max: f32,
    roughness: f32,
) -> vec4<f32> {
    let reflect_dir = reflect(-view_dir, world_normal);
    let ray_origin = world_pos + world_normal * 0.02;

    let max_dist: f32 = 30.0;
    let steps: i32 = 32;
    let step_size = max_dist / f32(steps);

    var ray_pos = ray_origin;

    for (var i: i32 = 0; i < steps; i = i + 1) {
        ray_pos = ray_pos + reflect_dir * step_size;

        let uvz = project_world_to_uv(ray_pos);
        if (uvz.z < 0.0) { return vec4<f32>(0.0); }
        if (uvz.x < 0.0 || uvz.x > 1.0 || uvz.y < 0.0 || uvz.y > 1.0) {
            return vec4<f32>(0.0);
        }

        let sample_depth = load_depth(uvz.xy);
        if (sample_depth >= 0.9999) {
            continue;
        }

        let sample_world = reconstruct_world_pos(uvz.xy, sample_depth);
        let ray_dist = length(ray_pos - ray_origin);
        let surface_dist = length(sample_world - ray_origin);

        if (surface_dist < ray_dist - 0.02) {
            let dist_fade = 1.0 - smoothstep(max_dist * 0.5, max_dist, ray_dist);
            let weight = fresnel_max * (1.0 - roughness) * dist_fade;
            let hit_color = textureSampleLevel(t_hdr, s_lin, uvz.xy, 0.0).rgb;
            return vec4<f32>(hit_color, weight);
        }
    }

    return vec4<f32>(0.0);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    // --- 1. Базовый HDR ----------------------------------------
    var hdr = textureSample(t_hdr, s_lin, in.uv).rgb;
    let depth_raw = load_depth(in.uv);

    // --- 2. SSR ------------------------------------------------
    if (depth_raw < 0.9999) {
        let g_normal = textureSample(t_normal, s_lin, in.uv);
        let g_albedo = textureSample(t_albedo, s_lin, in.uv);
        let g_emissive = textureSample(t_emissive, s_lin, in.uv);

        let world_normal = normalize(g_normal.xyz);
        let metallic = clamp(g_albedo.a, 0.0, 1.0);
        let roughness = clamp(g_emissive.a, 0.04, 1.0);
        let albedo = g_albedo.rgb;

        let world_pos = reconstruct_world_pos(in.uv, depth_raw);
        let view_dir = normalize(camera.camera_pos.xyz - world_pos);
        // clamp верхней границы обязателен: dot(n, v) может дать
        // 1.00001 при численной ошибке → pow(отрицательного, 5) = NaN.
        let n_dot_v = clamp(dot(world_normal, view_dir), 1e-4, 1.0);

        let f0 = mix(vec3<f32>(0.04), albedo, metallic);
        let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - n_dot_v, 0.0, 1.0), 5.0);
        let fresnel_max = max(max(fresnel.x, fresnel.y), fresnel.z);

        let refl_strength = fresnel_max * (1.0 - roughness);
        if (refl_strength > 0.02) {
            let ssr = trace_ssr(
                world_pos, world_normal, view_dir,
                fresnel_max, roughness,
            );
            hdr = hdr + ssr.rgb * ssr.a;
        }
    }

    // --- 3. Fog composite --------------------------------------
    if (vol.params.x <= 0.0) {
        return vec4<f32>(hdr, 1.0);
    }

    let near = vol.cam.x;
    let far = vol.cam.y;

    let view_z = (near * far) / max(far - depth_raw * (far - near), 1e-4);
    let ratio = far / near;
    let slice_f = clamp(log(view_z / near) / log(ratio), 0.0, 1.0);

    let z_uv = slice_f * (FROXEL_Z - 1.0) / FROXEL_Z + 0.5 / FROXEL_Z;
    let fog = textureSampleLevel(
        t_fog,
        s_lin,
        vec3<f32>(in.uv.x, in.uv.y, z_uv),
        0.0,
    );

    let scattering = fog.rgb;
    let extinction = fog.a;
    let transmittance = exp(-extinction);

    let result = hdr * transmittance + scattering;
    return vec4<f32>(result, 1.0);
}