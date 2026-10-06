struct FaceUniform {
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
};

@group(0) @binding(0) var t_equirect: texture_2d<f32>;
@group(0) @binding(1) var s_equirect: sampler;
@group(0) @binding(2) var<uniform> face: FaceUniform;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec3<f32>,
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
    out.local = vec3<f32>(p.x, -p.y, 1.0);
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let dir = normalize(
        face.right.xyz * in.local.x +
        face.up.xyz * in.local.y +
        face.forward.xyz * in.local.z
    );
    // equirect: u = atan2(z, x), v = asin(y)
    let u = atan2(dir.z, dir.x) / 6.28318530718 + 0.5;
    let v = asin(clamp(dir.y, -1.0, 1.0)) / 3.14159265359 + 0.5;
    return vec4<f32>(textureSample(t_equirect, s_equirect, vec2<f32>(u, v)).rgb, 1.0);
}