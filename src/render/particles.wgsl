struct Camera {
    view_proj:     mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view:          mat4x4<f32>,
    inv_view:      mat4x4<f32>,
    camera_pos:    vec4<f32>,
    near_far:      vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct Instance {
    @location(0) position_size: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    inst: Instance,
) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>( 0.5,  0.5),
        vec2<f32>(-0.5, -0.5),
        vec2<f32>( 0.5,  0.5),
        vec2<f32>(-0.5,  0.5),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );

    let c = corners[vi];

    // view — column-major: view[0] = x-axis (right), view[1] = y-axis (up).
    let right = camera.view[0].xyz;
    let up    = camera.view[1].xyz;

    let size = inst.position_size.w;
    let world = inst.position_size.xyz
        + right * c.x * size
        + up    * c.y * size;

    var out: VOut;
    out.clip  = camera.view_proj * vec4<f32>(world, 1.0);
    out.uv    = uvs[vi];
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    // Мягкий радиальный градиент из UV — не требует текстуры.
    let d = length(in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let falloff = clamp(1.0 - d, 0.0, 1.0);
    let soft = falloff * falloff;
    let alpha = in.color.a * soft;
    if (alpha < 0.003) { discard; }
    return vec4<f32>(in.color.rgb, alpha);
}