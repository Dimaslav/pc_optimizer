// Deferred decal pass.

struct Camera {
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    view:           mat4x4<f32>,
    inv_view:       mat4x4<f32>,
    camera_pos:     vec4<f32>,
    near_far:       vec4<f32>,
    prev_view_proj: mat4x4<f32>,
    screen_size:    vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@group(1) @binding(0) var t_decal: texture_2d<f32>;
@group(1) @binding(1) var s_decal: sampler;

@group(2) @binding(0) var t_scene_depth: texture_depth_2d;

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
    @location(10) i0: vec4<f32>,
    @location(11) i1: vec4<f32>,
    @location(12) i2: vec4<f32>,
    @location(13) i3: vec4<f32>,
    @location(14) tint: vec4<f32>,
};

struct VsOut {
    @builtin(position) @invariant frag_coord: vec4<f32>,
    @location(0) i0: vec4<f32>,
    @location(1) i1: vec4<f32>,
    @location(2) i2: vec4<f32>,
    @location(3) i3: vec4<f32>,
    @location(4) tint: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let world = model * vec4<f32>(in.position, 1.0);

    var out: VsOut;
    out.frag_coord = camera.view_proj * world;
    out.i0 = in.i0;
    out.i1 = in.i1;
    out.i2 = in.i2;
    out.i3 = in.i3;
    out.tint = in.tint;
    return out;
}

fn load_scene_depth(uv: vec2<f32>) -> f32 {
    let dim = vec2<f32>(textureDimensions(t_scene_depth));
    let pixel = vec2<i32>(uv * dim);
    let clamped = clamp(pixel, vec2<i32>(0, 0), vec2<i32>(dim) - vec2<i32>(1, 1));
    return textureLoad(t_scene_depth, clamped, 0);
}

fn reconstruct_world_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
    let world_h = camera.inv_view_proj * ndc;
    return world_h.xyz / max(world_h.w, 1e-6);
}

struct FsOut {
    @location(0) albedo:   vec4<f32>,
    @location(1) normal_d: vec4<f32>,
    @location(2) emissive: vec4<f32>,
    @location(3) motion:   vec2<f32>,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let uv = in.frag_coord.xy / camera.screen_size.xy;

    let scene_depth = load_scene_depth(uv);
    if (scene_depth >= 0.9999) {
        discard;
    }

    let scene_world = reconstruct_world_pos(uv, scene_depth);

    let inv_model = mat4x4<f32>(in.i0, in.i1, in.i2, in.i3);
    let local = (inv_model * vec4<f32>(scene_world, 1.0)).xyz;

    if (abs(local.x) > 0.5 || abs(local.y) > 0.5 || abs(local.z) > 0.5) {
        discard;
    }

    let dx = 0.5 - abs(local.x);
    let dy = 0.5 - abs(local.y);
    let dz = 0.5 - abs(local.z);

    var decal_uv: vec2<f32>;
    if (dy <= dx && dy <= dz) {
        decal_uv = vec2<f32>(local.x + 0.5, local.z + 0.5);
    } else if (dx <= dz) {
        decal_uv = vec2<f32>(local.z + 0.5, local.y + 0.5);
    } else {
        decal_uv = vec2<f32>(local.x + 0.5, local.y + 0.5);
    }

    let tex = textureSample(t_decal, s_decal, decal_uv);

    let edge = clamp(min(min(dx, dy), dz) * 8.0, 0.0, 1.0);
    let alpha = tex.a * in.tint.a * edge;
    if (alpha < 0.005) {
        discard;
    }

    let rgb = tex.rgb * in.tint.rgb;

    return FsOut(
        vec4<f32>(rgb, alpha),
        vec4<f32>(0.0),
        vec4<f32>(0.0),
        vec2<f32>(0.0),
    );
}