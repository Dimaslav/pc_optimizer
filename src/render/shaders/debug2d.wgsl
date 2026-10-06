struct DebugParams {
    mode: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
@group(0) @binding(2) var<uniform> params: DebugParams;

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
    let c = textureSample(t_src, s_src, in.uv);
    switch (params.mode) {
        case 1u: { return vec4<f32>(vec3<f32>(c.r), 1.0); }
        case 2u: { return vec4<f32>(c.rgb * 0.5 + 0.5, 1.0); }
        case 3u: { return vec4<f32>(vec3<f32>(c.a), 1.0); }  // depth из .a normal texture
        default: { return vec4<f32>(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0); }
    }
}