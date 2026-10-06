// Отладочный вывод depth_2d_array (CSM каскад).

@group(0) @binding(0) var t_depth: texture_depth_2d_array;
@group(0) @binding(1) var s_lin: sampler;

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

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let d = textureSample(t_depth, s_lin, in.uv, 0);
    return vec4<f32>(vec3<f32>(d), 1.0);
}