const PI: f32 = 3.14159265359;

struct FaceUniform {
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    params: vec4<f32>,  // x = roughness, y = env_size
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

fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    return mat3x3<f32>(t, b, n);
}

// Importance sampling по GGX
fn importance_sample_ggx(xi: vec2<f32>, n: vec3<f32>, roughness: f32) -> vec3<f32> {
    let a = roughness * roughness;
    let phi = 2.0 * PI * xi.x;
    let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_theta = sqrt(1.0 - cos_theta * cos_theta);

    let h = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
    let b = basis(n);
    return normalize(b * h);
}

fn radical_inverse_vdc(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

fn hammersley(i: u32, n: u32) -> vec2<f32> {
    return vec2<f32>(f32(i) / f32(n), radical_inverse_vdc(i));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let n = normalize(
        face.right.xyz * in.local.x +
        face.up.xyz * in.local.y +
        face.forward.xyz * in.local.z
    );
    let v = n;
    let roughness = face.params.x;

    var sum = vec3<f32>(0.0);
    var total_weight = 0.0;
    const SAMPLE_COUNT: u32 = 64u;

    for (var i: u32 = 0u; i < SAMPLE_COUNT; i = i + 1u) {
        let xi = hammersley(i, SAMPLE_COUNT);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(2.0 * dot(v, h) * h - v);

        let n_dot_l = max(dot(n, l), 0.0);
        if (n_dot_l > 0.0) {
            sum += textureSampleLevel(t_env, s_env, l, 0.0).rgb * n_dot_l;
            total_weight += n_dot_l;
        }
    }

    let result = sum / max(total_weight, 0.001);
    return vec4<f32>(result, 1.0);
}