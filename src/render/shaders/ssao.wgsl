struct SsaoUniform {
    // x = proj[0][0], y = proj[1][1], z = far, w = radius
    proj_scale: vec4<f32>,
    // x = bias, y = strength, zw = noise_uv_scale (screen_w/4, screen_h/4)
    params: vec4<f32>,
    // x = time (для temporal rotation шума)
    time: vec4<f32>,
    // world → view. Нужна, потому что G-buffer хранит world-space нормаль,
    // а SSAO восстанавливает view-space позицию и сэмплит hemisphere
    // в view-space. Без этого преобразования нормаль была бы «в другой
    // системе координат», TBN строился бы неверно, AO-паттерн поехал бы.
    view: mat4x4<f32>,
};

@group(0) @binding(0) var t_gbuffer: texture_2d<f32>;
@group(0) @binding(1) var t_noise: texture_2d<f32>;
@group(0) @binding(2) var s_lin: sampler;
@group(0) @binding(3) var<uniform> params: SsaoUniform;
// binding 4:
//   для ssao pass — тот же gbuffer (не используется, но должен быть привязан);
//   для blur pass — gbuffer (для bilateral depth weight).
@group(0) @binding(4) var t_depth_src: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
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
    var out: VertexOutput;
    out.clip = vec4<f32>(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

// Восстанавливает view-space позицию из UV и нормализованной глубины.
fn reconstruct_view_pos(uv: vec2<f32>, depth_norm: f32) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0);
    let ld = depth_norm * params.proj_scale.z;
    let vx = ndc.x * ld / params.proj_scale.x;
    let vy = ndc.y * ld / params.proj_scale.y;
    return vec3<f32>(vx, vy, -ld);
}

// 2D-поворот UV.
fn rotate_uv(v: vec2<f32>, angle: f32) -> vec2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(c * v.x - s * v.y, s * v.x + c * v.y);
}

// Проекция view-space точки в UV экрана.
//
// ИЗМЕНЕНО (#5): раньше здесь было `clip.w = 1.0`, из-за чего
// перспективное деление отсутствовало. Правильная формула:
//
//   ndc.x = sp.x * proj[0][0] / (-sp.z)
//   ndc.y = sp.y * proj[1][1] / (-sp.z)
//
// `-sp.z` — линейная глубина (view-space Z идёт в минус), тот же
// самый `ld`, что использует `reconstruct_view_pos` в обратную
// сторону. Без этого деления sample-точки уезжали от экранной
// позиции фрагмента пропорционально глубине: на 1 м ошибка почти
// нулевая, на 10 м — sample за пределами вьюпорта, SSAO фактически
// не работал.
//
// Защита: `w = max(-sp.z, 1e-4)` — если точка оказалась позади
// near-plane (например, при большом radius от очень близкого
// фрагмента), делим на маленькое положительное число вместо
// нуля. Результат в таком случае заведомо вне [0,1] по UV, и
// вызывающий код вернёт 0 через range-check.
fn project_view_to_uv(sp: vec3<f32>) -> vec2<f32> {
    let w = max(-sp.z, 1e-4);
    let ndc_x = sp.x * params.proj_scale.x / w;
    let ndc_y = sp.y * params.proj_scale.y / w;
    return vec2<f32>(ndc_x * 0.5 + 0.5, 0.5 - ndc_y * 0.5);
}

// Один сэмпл: проекция hemisphere-точки в экран, чтение depth,
// сравнение с реальной depth сцены, возврат 1 если окклюзия.
fn sample_occlusion(
    tbn: mat3x3<f32>,
    dir_ts: vec3<f32>,
    view_pos: vec3<f32>,
    radius: f32,
    bias: f32,
) -> f32 {
    let s = tbn * normalize(dir_ts) * radius;
    let sp = view_pos + s;

    // ИЗМЕНЕНО (#5): используем корректную проекцию вместо
    // сломанной `clip.w = 1.0`. `bias` в старом коде вычитался из
    // `clip.z`, что не влияло на UV (мы берём только `.xy / .w`).
    // Здесь bias применяется только к сравнению глубин ниже —
    // так и должно быть.
    let suv = project_view_to_uv(sp);

    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
        return 0.0;
    }

    let sd = textureSample(t_depth_src, s_lin, suv).a * params.proj_scale.z;
    let sample_z = -sd;

    // Range-check: только близкие окклюзии (без halo).
    //
    // `view_pos.z` и `sample_z` — отрицательные (view-space Z).
    // Условие `sample_z > sp.z + bias` означает: sample ближе к
    // камере, чем fragment + bias. Это и есть окклюзия.
    // Второе условие ограничивает окклюзию радиусом.
    if (sample_z > sp.z + bias && sample_z < sp.z + radius * 2.0) {
        return 1.0;
    }
    return 0.0;
}

// 16 сэмплов в hemisphere — развёрнуто, потому что naga
// не разрешает индексировать array переменной.
fn compute_occlusion(view_pos: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>) -> f32 {
    let radius = params.proj_scale.w;
    let bias = params.params.x;

    // Temporal rotation: шум поворачивается по времени → нет фиксированного pattern.
    let angle = params.time.x * 5.0;
    let noise_uv = fract(rotate_uv(uv * params.params.zw, angle));
    let noise = textureSample(t_noise, s_lin, noise_uv).xyz;

    // Gram-Schmidt: T ортогонален N.
    var tangent = noise - normal * dot(noise, normal);
    if (length(tangent) < 0.001) {
        tangent = normalize(cross(normal, vec3<f32>(0.0, 1.0, 0.0)));
    }
    tangent = normalize(tangent);
    let bitangent = cross(normal, tangent);
    let tbn = mat3x3<f32>(tangent, bitangent, normal);

    var occlusion = 0.0;

    // 16 сэмплов по hemisphere.
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.2023,  0.1348,  0.9704), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.1829,  0.3449,  0.9206), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.3075, -0.4656,  0.8300), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.4520, -0.3134,  0.8351), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.6108,  0.6121,  0.5022), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.7742,  0.4406,  0.4544), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.7227, -0.5960,  0.3505), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.6709, -0.6423,  0.3706), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.1714,  0.1323,  0.9764), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.2906,  0.4864,  0.8245), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.4703, -0.6807,  0.5614), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.5925, -0.5552,  0.5839), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.7548,  0.5912,  0.2849), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.8320,  0.4464,  0.3304), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>( 0.8313, -0.4696,  0.2992), view_pos, radius, bias);
    occlusion += sample_occlusion(tbn, vec3<f32>(-0.7555, -0.5623,  0.3350), view_pos, radius, bias);

    return occlusion / 16.0;
}

@fragment
fn fs_ssao(in: VertexOutput) -> @location(0) vec4<f32> {
    let g = textureSample(t_gbuffer, s_lin, in.uv);
    let depth_norm = g.a;
    if (depth_norm >= 0.9999) {
        return vec4<f32>(1.0, 0.0, 0.0, 1.0);
    }
    // G-buffer хранит WORLD-space нормаль. SSAO работает во view-space
    // (reconstruct_view_pos + hemisphere-сэмплы + проекция). Приводим
    // нормаль к view-space через camera.view.
    let n_world = normalize(g.xyz);
    let normal = normalize((params.view * vec4<f32>(n_world, 0.0)).xyz);
    let view_pos = reconstruct_view_pos(in.uv, depth_norm);
    let ao = compute_occlusion(view_pos, normal, in.uv);
    return vec4<f32>(1.0 - ao, 0.0, 0.0, 1.0);
}

// ============================================================
// Bilateral blur 5×5
// ============================================================
// Weight = exp(-depth_diff² · k) · exp(-xy_dist² / 2).
// При большом расхождении глубин weight → 0, поэтому края не размываются.
@fragment
fn fs_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(t_gbuffer));
    let center_g = textureSample(t_depth_src, s_lin, in.uv);
    let center_depth = center_g.a;

    var sum = 0.0;
    var w_sum = 0.0;

    for (var y = -2; y <= 2; y = y + 1) {
        for (var x = -2; x <= 2; x = x + 1) {
            let o = vec2<f32>(f32(x), f32(y));
            let suv = in.uv + o * texel;

            // `t_gbuffer` в blur-пасе — это raw SSAO-текстура,
            // а `t_depth_src` — G-buffer (для depth-веса). Раньше
            // переменная называлась `sample_v`, что сбивало с толку.
            let ssao_sample = textureSample(t_gbuffer, s_lin, suv);
            let sample_depth = textureSample(t_depth_src, s_lin, suv).a;

            let depth_diff = abs(sample_depth - center_depth);
            let depth_w = exp(-depth_diff * depth_diff * 8000.0);

            let dist2 = dot(o, o);
            let spatial_w = exp(-dist2 * 0.5);

            let w = depth_w * spatial_w;
            sum += ssao_sample.r * w;
            w_sum += w;
        }
    }

    if (w_sum > 0.0) {
        sum /= w_sum;
    }
    return vec4<f32>(sum, 0.0, 0.0, 1.0);
}