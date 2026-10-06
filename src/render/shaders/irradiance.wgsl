const PI: f32 = 3.14159265359;

struct FaceUniform {
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
};

@group(0) @binding(0) var t_env: texture_cube<f32>;
@group(0) @binding(1) var s_env: sampler;
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

// Строим tangent space вокруг N
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let n = normalize(
        face.right.xyz * in.local.x +
        face.up.xyz * in.local.y +
        face.forward.xyz * in.local.z
    );
    let b = basis(n);

    var sum = vec3<f32>(0.0);
    var count = 0.0;
    let sample_delta = 0.025;

    // Полусферическая конволюция через сэмплирование
    for (var phi = 0.0; phi < 6.28318530718; phi = phi + 0.20) {
        for (var theta = 0.0; theta < 1.57079632679; theta = theta + 0.10) {
            let l = b * vec3<f32>(
                sin(theta) * cos(phi),
                sin(theta) * sin(phi),
                cos(theta)
            );
            sum += textureSampleLevel(t_env, s_env, l, 0.0).rgb * cos(theta) * sin(theta);
            count += 1.0;
        }
    }

    let result = PI * sum / max(count, 1.0);
    return vec4<f32>(result, 1.0);
}