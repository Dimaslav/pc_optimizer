// Forward transparent. 2 MRT: HDR + motion.
// Motion — как в gbuffer: prev_vp **unjittered**, база = pixel_center_uv_unjit.
//
// pixel_center_uv_unjit = (frag_coord.xy - camera.near_far.zw) / screen_size,
// где near_far.zw = jitter текущего кадра в пикселях.

struct Camera {
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    view:           mat4x4<f32>,
    inv_view:       mat4x4<f32>,
    camera_pos:     vec4<f32>,
    // xy = (near, far), zw = (jitter_x_px, jitter_y_px) текущего кадра.
    near_far:       vec4<f32>,
    prev_view_proj: mat4x4<f32>,
    screen_size:    vec4<f32>,
};

struct Lights {
    cascade_vp:      array<mat4x4<f32>, 3>,
    cascade_splits:  vec4<f32>,
    ambient_color:   vec4<f32>,
    // x = dir_count, y = point_count, z = cube_shadow_count, w = unused.
    // ИЗМЕНЕНО (#7): раньше z всегда был 0/1; теперь — фактическое
    // число активных cube shadow maps. Шейдер всё равно не читает
    // тени, но layout должен совпадать.
    counts:          vec4<u32>,
    light_view_proj: mat4x4<f32>,
    misc:            vec4<f32>,
    fog_params:      vec4<f32>,
    fog_color:       vec4<f32>,
    // ИЗМЕНЕНО (#7): переименовано `_pad1` → `shadow_params`.
    // Слот содержит (depth_bias, normal_bias, fade_start, fade_end).
    // Раньше комментарий вводил в заблуждение — в `deferred_lighting.wgsl`
    // и `volumetric_fog.wgsl` тот же слот называется `shadow_params`.
    shadow_params:   vec4<f32>,
    dir_lights:      array<vec4<f32>, 8>,
    point_lights:    array<vec4<f32>, 32>,
    cube_shadow_pos: array<vec4<f32>, 4>,
};

struct Material {
    base_color: vec4<f32>,
    emissive:   vec4<f32>,
    params:     vec4<f32>,
    flags:      vec4<u32>,
};

struct Skeleton {
    joints: array<mat4x4<f32>, 64>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> lights: Lights;

@group(2) @binding(0)  var csm_tex:          texture_depth_2d_array;
@group(2) @binding(1)  var csm_samp:         sampler_comparison;
// ИЗМЕНЕНО (#7): `texture_depth_cube` → `texture_depth_cube_array`.
// Раньше был один куб (только для первого point light), теперь —
// массив `6 * MAX_SHADOW_CUBES` слоёв. Шейдер сам не читает cube
// shadow, но тип binding'а обязан совпадать с `shadow2_layout`.
@group(2) @binding(2)  var cube_shadow_tex:  texture_depth_cube_array;
@group(2) @binding(3)  var cube_shadow_samp: sampler_comparison;
@group(2) @binding(4)  var ssao_tex:         texture_2d<f32>;
@group(2) @binding(5)  var linear_samp:      sampler;
@group(2) @binding(6)  var irradiance_tex:   texture_cube<f32>;
@group(2) @binding(7)  var prefiltered_tex:  texture_cube<f32>;
@group(2) @binding(8)  var brdf_lut_tex:     texture_2d<f32>;
@group(2) @binding(9)  var env_samp:         sampler;
@group(2) @binding(10) var env_cube_tex:     texture_cube<f32>;

@group(3) @binding(0) var base_tex:     texture_2d<f32>;
@group(3) @binding(1) var mr_tex:       texture_2d<f32>;
@group(3) @binding(2) var normal_tex:   texture_2d<f32>;
@group(3) @binding(3) var emissive_tex: texture_2d<f32>;
@group(3) @binding(4) var mat_samp:     sampler;
@group(3) @binding(5) var<uniform> mat: Material;
@group(3) @binding(6) var<uniform> skeleton: Skeleton;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal:   vec3<f32>,
    @location(2) uv:       vec2<f32>,
    @location(3) color:    vec4<f32>,
    @location(4) joints:   vec4<u32>,
    @location(5) weights:  vec4<f32>,
    @location(6)  m0: vec4<f32>,
    @location(7)  m1: vec4<f32>,
    @location(8)  m2: vec4<f32>,
    @location(9)  m3: vec4<f32>,
    @location(10) n0: vec4<f32>,
    @location(11) n1: vec4<f32>,
    @location(12) n2: vec4<f32>,
    @location(13) n3: vec4<f32>,
    @location(14) inst_color: vec4<f32>,
    @location(15) uv_scale:   vec4<f32>,
    @location(16) p0: vec4<f32>,
    @location(17) p1: vec4<f32>,
    @location(18) p2: vec4<f32>,
    @location(19) p3: vec4<f32>,
};

struct VsOut {
    @builtin(position) frag_coord: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) prev_world_pos: vec3<f32>,
};

fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let w_sum = weights.x + weights.y + weights.z + weights.w;
    if (w_sum < 0.0001) {
        return mat4x4<f32>(
            vec4<f32>(1.0, 0.0, 0.0, 0.0),
            vec4<f32>(0.0, 1.0, 0.0, 0.0),
            vec4<f32>(0.0, 0.0, 1.0, 0.0),
            vec4<f32>(0.0, 0.0, 0.0, 1.0),
        );
    }
    return weights.x * skeleton.joints[joints.x]
         + weights.y * skeleton.joints[joints.y]
         + weights.z * skeleton.joints[joints.z]
         + weights.w * skeleton.joints[joints.w];
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let nrm_mat = mat4x4<f32>(in.n0, in.n1, in.n2, in.n3);
    let prev_model = mat4x4<f32>(in.p0, in.p1, in.p2, in.p3);

    var local_pos = vec4<f32>(in.position, 1.0);
    var local_nrm = vec4<f32>(in.normal, 0.0);

    let w_sum = in.weights.x + in.weights.y + in.weights.z + in.weights.w;
    if (w_sum > 0.0001) {
        let sk = skin_matrix(in.joints, in.weights);
        local_pos = sk * local_pos;
        local_nrm = sk * local_nrm;
    }

    let world_pos = model * local_pos;
    let prev_world_pos = (prev_model * local_pos).xyz;
    let world_nrm = normalize((nrm_mat * local_nrm).xyz);

    var out: VsOut;
    out.frag_coord      = camera.view_proj * world_pos;
    out.world_pos       = world_pos.xyz;
    out.world_normal    = world_nrm;
    out.uv              = in.uv * in.uv_scale.xy;
    out.color           = in.color * in.inst_color;
    out.prev_world_pos  = prev_world_pos;
    return out;
}

fn cotangent_frame(N: vec3<f32>, p: vec3<f32>, uv: vec2<f32>) -> mat3x3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);

    let dp2perp = cross(dp2, N);
    let dp1perp = cross(N, dp1);
    let T = dp2perp * duv1.x + dp1perp * duv2.x;
    let B = dp2perp * duv1.y + dp1perp * duv2.y;

    let invmax = inverseSqrt(max(max(dot(T, T), dot(B, B)), 0.0001));
    return mat3x3<f32>(T * invmax, B * invmax, N);
}

fn dir_contrib(n: vec3<f32>, idx: u32) -> vec3<f32> {
    let dir_p = lights.dir_lights[idx * 2u];
    let col_p = lights.dir_lights[idx * 2u + 1u];
    let l = normalize(dir_p.xyz);
    let ndl = max(dot(n, l), 0.0);
    return col_p.rgb * dir_p.w * ndl;
}

fn point_contrib(n: vec3<f32>, world_pos: vec3<f32>, idx: u32) -> vec3<f32> {
    let pos_range = lights.point_lights[idx * 2u];
    let col_int   = lights.point_lights[idx * 2u + 1u];
    let to_l = pos_range.xyz - world_pos;
    let dist = length(to_l);
    if (dist > pos_range.w) { return vec3<f32>(0.0); }
    let l = to_l / max(dist, 0.0001);
    let ndl = max(dot(n, l), 0.0);
    let atten = clamp(1.0 - dist / pos_range.w, 0.0, 1.0);
    return col_int.rgb * col_int.a * ndl * atten * atten;
}

fn project_to_uv(vp: mat4x4<f32>, world_pos: vec3<f32>) -> vec2<f32> {
    let clip = vp * vec4<f32>(world_pos, 1.0);
    let inv_w = 1.0 / max(abs(clip.w), 1e-6) * sign(clip.w);
    let ndc = clip.xy * inv_w;
    return vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
}

struct FsOut {
    @location(0) color:  vec4<f32>,
    @location(1) motion: vec2<f32>,
};

@fragment
fn fs_main(
    in: VsOut,
    @builtin(front_facing) front_facing: bool,
) -> FsOut {
    let base_sample     = textureSample(base_tex,     mat_samp, in.uv);
    let mr_sample       = textureSample(mr_tex,       mat_samp, in.uv);
    let normal_sample   = textureSample(normal_tex,   mat_samp, in.uv);
    let emissive_sample = textureSample(emissive_tex, mat_samp, in.uv);

    let base = base_sample * mat.base_color * in.color;

    let alpha = base.a;
    if (alpha < 0.003) { discard; }

    var n_world = normalize(in.world_normal);
    if (!front_facing) {
        n_world = -n_world;
    }

    let n_sample = normal_sample.xyz * 2.0 - 1.0;
    let n_scaled = vec3<f32>(n_sample.xy * mat.params.z, n_sample.z);
    let tbn = cotangent_frame(n_world, in.world_pos, in.uv);
    let n = normalize(tbn * n_scaled);

    let emissive_rgb = emissive_sample.rgb * mat.emissive.rgb;

    let irr = textureSample(irradiance_tex, env_samp, n).rgb;
    let ambient = lights.ambient_color.rgb;
    let ibl_strength = lights.misc.x;

    var lit = vec3<f32>(0.0);
    let dir_count = lights.counts.x;
    for (var i = 0u; i < dir_count; i = i + 1u) {
        lit = lit + dir_contrib(n, i);
    }
    let pt_count = lights.counts.y;
    for (var i = 0u; i < pt_count; i = i + 1u) {
        lit = lit + point_contrib(n, in.world_pos, i);
    }

    let diffuse = base.rgb;
    var out_rgb = diffuse * (ambient + irr * ibl_strength + lit) + emissive_rgb;

    let fog_density = lights.fog_params.x;
    if (fog_density > 0.0) {
        let cam_pos = camera.camera_pos.xyz;
        let dist = length(in.world_pos - cam_pos);
        let h = max(0.0, in.world_pos.y - lights.fog_params.y);
        let h_factor = exp(-h * lights.fog_params.z);
        let fog_amount = clamp(1.0 - exp(-dist * fog_density * h_factor), 0.0, 1.0);
        out_rgb = mix(out_rgb, lights.fog_color.rgb, fog_amount);
    }

    let pixel_center_uv = (in.frag_coord.xy - camera.near_far.zw) / camera.screen_size.xy;
    let prev_uv = project_to_uv(camera.prev_view_proj, in.prev_world_pos);
    let motion  = prev_uv - pixel_center_uv;

    var out: FsOut;
    out.color  = vec4<f32>(out_rgb, alpha);
    out.motion = motion;
    return out;
}