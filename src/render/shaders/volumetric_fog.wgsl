// Froxel-based volumetric fog (compute).
//
// Сетка 160×90×64. XY = screen space, Z = экспоненциальные depth-срезы.
// Для каждого froxel'я raymarch вдоль view-ray этого пикселя в этом
// depth-диапазоне, на каждом шаге:
//   1. Sample CSM shadow → light visibility
//   2. Накопить scattering = phase * sun_color * shadow * density * dt
//   3. Накопить extinction = density * dt
//
// Результат пишем в storage-texture 3D: rgb = scattering, a = extinction.
// Composite-pass потом применяет `hdr * exp(-ext) + scattering` к HDR.

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

struct Volumetric {
    grid: vec4<f32>,
    cam: vec4<f32>,
    params: vec4<f32>,
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> lights: Lights;
@group(0) @binding(2) var<uniform> vol: Volumetric;
@group(0) @binding(3) var t_csm: texture_depth_2d_array;
@group(0) @binding(4) var s_csm: sampler_comparison;
@group(0) @binding(5) var t_fog_out: texture_storage_3d<rgba16float, write>;

const FROXEL_Z: u32 = 64u;
const MARCH_STEPS: u32 = 32u;
const PI: f32 = 3.14159265359;

fn sample_csm_soft(world_pos: vec3<f32>, cascade: i32, bias: f32) -> f32 {
    let light_clip = lights.cascade_vp[cascade] * vec4<f32>(world_pos, 1.0);
    let ndc = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);

    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) { return 1.0; }
    if (ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }

    let depth = ndc.z - bias;

    var shadow = 0.0;
    shadow += textureSampleCompareLevel(t_csm, s_csm, uv + vec2<f32>(-0.0025, -0.0015), cascade, depth);
    shadow += textureSampleCompareLevel(t_csm, s_csm, uv + vec2<f32>( 0.0015, -0.0025), cascade, depth);
    shadow += textureSampleCompareLevel(t_csm, s_csm, uv + vec2<f32>( 0.0025,  0.0015), cascade, depth);
    shadow += textureSampleCompareLevel(t_csm, s_csm, uv + vec2<f32>(-0.0015,  0.0025), cascade, depth);
    return shadow * 0.25;
}

fn compute_csm_shadow(world_pos: vec3<f32>, view_depth: f32, l: vec3<f32>) -> f32 {
    let s = lights.shadow_params;
    let bias = s.x * 4.0;

    var cascade = 0;
    if (view_depth > lights.cascade_splits.x) { cascade = 1; }
    if (view_depth > lights.cascade_splits.y) { cascade = 2; }

    var shadow = sample_csm_soft(world_pos, cascade, bias);

    let fade_start = s.z;
    let fade_end = s.w;
    if (view_depth > fade_start) {
        let fade = clamp((view_depth - fade_start) / max(fade_end - fade_start, 1e-4), 0.0, 1.0);
        shadow = mix(shadow, 1.0, fade);
    }
    return shadow;
}

fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    let denom = 1.0 + g2 - 2.0 * g * cos_theta;
    return (1.0 - g2) / (4.0 * PI * pow(max(denom, 1e-4), 1.5));
}

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 = p3 + vec3<f32>(dot(p3, p3.zyx + vec3<f32>(31.32)));
    return fract((p3.x + p3.y) * p3.z);
}

@compute @workgroup_size(8, 8, 4)
fn cs_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let grid_x = u32(vol.grid.x);
    let grid_y = u32(vol.grid.y);
    if (gid.x >= grid_x || gid.y >= grid_y || gid.z >= FROXEL_Z) { return; }

    if (vol.params.x <= 0.0) {
        textureStore(t_fog_out, vec3<i32>(gid), vec4<f32>(0.0, 0.0, 0.0, 0.0));
        return;
    }

    let uv = (vec2<f32>(gid.xy) + 0.5) / vec2<f32>(f32(grid_x), f32(grid_y));
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0);

    let near = vol.cam.x;
    let far = vol.cam.y;
    let tan_half = vol.cam.z;
    let aspect = vol.cam.w;

    let zf = f32(gid.z);
    let zn = f32(FROXEL_Z);
    let ratio = far / near;

    let z0 = near * pow(ratio, zf / zn);
    let z1 = near * pow(ratio, (zf + 1.0) / zn);

    let x0 = ndc.x * z0 * tan_half * aspect;
    let y0 = ndc.y * z0 * tan_half;
    let x1 = ndc.x * z1 * tan_half * aspect;
    let y1 = ndc.y * z1 * tan_half;

    let p0_view = vec3<f32>(x0, y0, -z0);
    let p1_view = vec3<f32>(x1, y1, -z1);

    let inv_view = camera.inv_view;
    let p0_world = (inv_view * vec4<f32>(p0_view, 1.0)).xyz;
    let p1_world = (inv_view * vec4<f32>(p1_view, 1.0)).xyz;

    let cam_pos = camera.camera_pos.xyz;
    let dir_world = normalize(p1_world - p0_world);
    let seg_len = length(p1_world - p0_world);
    let step_len = seg_len / f32(MARCH_STEPS);

    let sun_dir_raw = lights.dir_lights[0].xyz;
    let sun_intensity = lights.dir_lights[0].w;
    let sun_color = lights.dir_lights[1].rgb;
    let sun_l = normalize(sun_dir_raw);

    let cos_theta = dot(dir_world, sun_l);
    let phase = hg_phase(cos_theta, vol.params.z);

    // Явный каст трёх f32, потому что `vec3<f32>(gid.xy, ...)` не
    // работает (gid.xy — vec2<u32>).
    let dither = (hash13(vec3<f32>(f32(gid.x), f32(gid.y), f32(gid.z))) - 0.5) * step_len;

    var scattering = vec3<f32>(0.0);
    var extinction = 0.0;

    let density = vol.params.x;
    let albedo = vol.params.y;
    let fog_tint = vol.fog_color.rgb;

    var t = dither;
    for (var i = 0u; i < MARCH_STEPS; i = i + 1u) {
        t = t + step_len;
        if (t > seg_len) { break; }

        let sample_pos = p0_world + dir_world * t;

        // View-space Z: трансформируем вектор-направление (w = 0)
        // матрицей вида и берём z. Раньше было `dot(to_sample,
        // camera.view[2].xyz)` — это третий СТОЛБЕЦ матрицы, не ось Z,
        // результат мусорный.
        let to_sample = sample_pos - cam_pos;
        let view_z = abs((camera.view * vec4<f32>(to_sample, 0.0)).z);
        let shadow = compute_csm_shadow(sample_pos, view_z, sun_l);

        let trans = exp(-extinction);

        let in_scatter = phase * sun_color * sun_intensity * albedo * density;
        scattering += fog_tint * in_scatter * shadow * step_len * trans;

        extinction += density * step_len;

        if (extinction > 6.0) { break; }
    }

    textureStore(
        t_fog_out,
        vec3<i32>(gid),
        vec4<f32>(scattering, extinction),
    );
}