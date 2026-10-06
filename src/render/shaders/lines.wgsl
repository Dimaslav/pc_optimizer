struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view: mat4x4<f32>,
    camera_pos: vec4<f32>,
    near_far: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}