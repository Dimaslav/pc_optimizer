// Shadow map pass: только глубина, без fragment shader.
// Используется и для CSM (ortho), и для cube shadow (6 faces).
//
// Bind groups: 0 = Lights (dynamic offset), 1 = material_layout
// (из которого берётся только skeleton).

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

struct Skeleton {
    joints: array<mat4x4<f32>, 64>,
};

@group(0) @binding(0) var<uniform> lights: Lights;
@group(1) @binding(6) var<uniform> skeleton: Skeleton;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal:   vec3<f32>,
    @location(2) uv:       vec2<f32>,
    @location(3) color:    vec4<f32>,
    @location(4) joints:   vec4<u32>,
    @location(5) weights:  vec4<f32>,
};

struct InstanceInput {
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
    // prev_model — не используется в shadow pass, но обязан
    // присутствовать: layout vertex-buffer'а общий с gbuffer.
    @location(16) p0: vec4<f32>,
    @location(17) p1: vec4<f32>,
    @location(18) p2: vec4<f32>,
    @location(19) p3: vec4<f32>,
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
fn vs_main(in: VertexInput, inst: InstanceInput) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);

    var local_pos = vec4<f32>(in.position, 1.0);
    let w_sum = in.weights.x + in.weights.y + in.weights.z + in.weights.w;
    if (w_sum > 0.0001) {
        let sk = skin_matrix(in.joints, in.weights);
        local_pos = sk * local_pos;
    }

    let world = model * local_pos;
    return lights.light_view_proj * world;
}