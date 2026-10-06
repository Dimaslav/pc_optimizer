struct Camera {
    view_proj:     mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view:          mat4x4<f32>,
    inv_view:      mat4x4<f32>,
    camera_pos:    vec4<f32>,
    near_far:      vec4<f32>,
};

struct Params {
    // x = ibl_strength, y = fog_density, z = fog_height_base, w = fog_height_falloff
    values: vec4<f32>,
    // rgb = fog_color
    fog_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var t_env: texture_cube<f32>;
@group(0) @binding(2) var s_env: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct VOut {
    // @invariant — no-op для текущего depth_compare: LessEqual
    // (см. make_skybox_pipeline). Оставлен как страховка, если
    // кто-то поменяет на Equal (тогда это станет обязательным).
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VOut;
    let p = positions[idx];
    out.clip = vec4<f32>(p, 1.0, 1.0);
    out.ndc = p;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let near_h = camera.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far_h  = camera.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let near_w = near_h.xyz / max(near_h.w, 1e-6);
    let far_w  = far_h.xyz  / max(far_h.w,  1e-6);
    let dir = normalize(far_w - near_w);

    var color = textureSampleLevel(t_env, s_env, dir, 0.0).rgb;
    color = color * params.values.x;

    let fog_density = params.values.y;
    if (fog_density > 0.0) {
        let horizon_t = clamp(1.0 - abs(dir.y), 0.0, 1.0);
        let fog_amount = clamp(horizon_t, 0.0, 1.0);
        color = mix(color, params.fog_color.rgb, fog_amount);
    }

    return vec4<f32>(color, 1.0);
}