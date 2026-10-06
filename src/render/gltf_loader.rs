//! Загрузчик glTF 2.0 с поддержкой PBR, skin и animation.

use anyhow::{bail, Context, Result};
use glam::Mat4;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::material::Material;
use super::mesh::{Mesh, Vertex3D};
use super::renderer::Renderer;
use super::skinning::{AnimationClip, Skeleton, Track, MAX_JOINTS, PARENT_NONE};

// ============================================================
// Публичные типы
// ============================================================

#[derive(Debug, Clone)]
pub struct GltfLoadOptions {
    pub prefix: String,
    pub load_textures: bool,
    pub load_animations: bool,
    pub max_joints: usize,
}

impl Default for GltfLoadOptions {
    fn default() -> Self {
        Self {
            prefix: "gltf".into(),
            load_textures: true,
            load_animations: true,
            max_joints: MAX_JOINTS,
        }
    }
}

pub struct GltfInstance {
    pub mesh_name: String,
    pub material_name: String,
    pub model: Mat4,
    pub skeleton_name: Option<String>,
    pub default_animation: Option<String>,
    pub node_name: Option<String>,
}

pub struct LoadedGltf {
    pub instances: Vec<GltfInstance>,
    pub skeletons: HashMap<String, Skeleton>,
    pub animations: HashMap<String, AnimationClip>,
    pub meshes: HashMap<String, usize>,
}

// ============================================================
// Точка входа
// ============================================================

pub fn load_gltf_into(
    renderer: &mut Renderer,
    path: impl AsRef<Path>,
    prefix: &str,
) -> Result<LoadedGltf> {
    let opts = GltfLoadOptions {
        prefix: prefix.into(),
        ..Default::default()
    };
    load_gltf_into_with(renderer, path, &opts)
}

pub fn load_gltf_into_with(
    renderer: &mut Renderer,
    path: impl AsRef<Path>,
    opts: &GltfLoadOptions,
) -> Result<LoadedGltf> {
    let path = path.as_ref();
    let (doc, buffers, images) = gltf::import(path)
        .with_context(|| format!("failed to import glTF: {}", path.display()))?;

    let prefix = &opts.prefix;

    // ============================================================
    // Шаг 0: категоризация изображений — sRGB vs linear
    // ============================================================
    let mut srgb_used: HashSet<usize> = HashSet::new();
    let mut linear_used: HashSet<usize> = HashSet::new();
    for mat in doc.materials() {
        let pbr = mat.pbr_metallic_roughness();
        if let Some(t) = pbr.base_color_texture() {
            srgb_used.insert(t.texture().source().index());
        }
        if let Some(t) = pbr.metallic_roughness_texture() {
            linear_used.insert(t.texture().source().index());
        }
        if let Some(t) = mat.normal_texture() {
            linear_used.insert(t.texture().source().index());
        }
        if let Some(t) = mat.emissive_texture() {
            srgb_used.insert(t.texture().source().index());
        }
    }

    // ============================================================
    // Шаг 1: текстуры
    // ============================================================
    let mut texture_name_map: HashMap<usize, String> = HashMap::new();
    if opts.load_textures {
        for (i, img) in images.iter().enumerate() {
            let name = format!("{}_tex_{}", prefix, i);
            let data = &img.pixels;
            let w = img.width;
            let h = img.height;

            let is_linear = linear_used.contains(&i) && !srgb_used.contains(&i);

            match img.format {
                gltf::image::Format::R8G8B8A8 => {
                    if is_linear {
                        renderer.load_texture_rgba_linear(&name, data, w, h)?;
                    } else {
                        renderer.load_texture_rgba(&name, data, w, h)?;
                    }
                }
                gltf::image::Format::R8G8B8 => {
                    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                    for px in data.chunks_exact(3) {
                        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                    if is_linear {
                        renderer.load_texture_rgba_linear(&name, &rgba, w, h)?;
                    } else {
                        renderer.load_texture_rgba(&name, &rgba, w, h)?;
                    }
                }
                _ => {
                    log::warn!(
                        "glTF: unsupported image format {:?} for '{}', skipping",
                        img.format,
                        name
                    );
                    continue;
                }
            }
            texture_name_map.insert(i, name);
        }
    }

    // ============================================================
    // Шаг 2: материалы
    // ============================================================
    let material_name_map = load_materials(renderer, &doc, prefix, &texture_name_map)?;

    // ============================================================
    // Шаг 3: скелеты
    // ============================================================
    let (skeletons, skeleton_by_skin_index) =
        load_skeletons(renderer, &doc, &buffers, prefix, opts.max_joints)?;

    // ============================================================
    // Шаг 4: дедуплицированные меши
    // ============================================================
    let mut mesh_name_map: HashMap<(usize, usize), String> = HashMap::new();
    let mut meshes_info: HashMap<String, usize> = HashMap::new();

    let mut used_primitives: HashSet<(usize, usize)> = HashSet::new();
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        for (p_idx, _) in mesh.primitives().enumerate() {
            used_primitives.insert((mesh.index(), p_idx));
        }
    }

    for (mesh_idx, p_idx) in &used_primitives {
        let mesh = doc.meshes().nth(*mesh_idx).expect("mesh index");
        let primitive = mesh.primitives().nth(*p_idx).expect("prim index");
        let name = format!("{}_mesh_{}_{}", prefix, mesh_idx, p_idx);
        let vertex_count = upload_primitive_mesh(renderer, &primitive, &buffers, &name)?;
        meshes_info.insert(name.clone(), vertex_count);
        mesh_name_map.insert((*mesh_idx, *p_idx), name);
    }

    // ============================================================
    // Шаг 5: world-матрицы нод
    // ============================================================
    let world_matrices = collect_world_transforms(&doc);

    // ============================================================
    // Шаг 6: инстансы
    // ============================================================
    let default_animation_name: Option<String> = if opts.load_animations {
        doc.animations()
            .next()
            .and_then(|a| a.name().map(|s| s.to_string()))
            .or_else(|| {
                if doc.animations().next().is_some() {
                    Some(format!("{}_anim_0", prefix))
                } else {
                    None
                }
            })
    } else {
        None
    };

    let mut instances = Vec::new();
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let world = world_matrices
            .get(&node.index())
            .copied()
            .unwrap_or_else(|| Mat4::from_cols_array_2d(&node.transform().matrix()));

        let node_skeleton_name: Option<String> = node
            .skin()
            .and_then(|skin| skeleton_by_skin_index.get(&skin.index()).cloned());

        for (p_idx, primitive) in mesh.primitives().enumerate() {
            let Some(mesh_name) = mesh_name_map.get(&(mesh.index(), p_idx)).cloned() else {
                continue;
            };

            let raw_material = primitive
                .material()
                .index()
                .and_then(|idx| material_name_map.get(&idx).cloned())
                .unwrap_or_else(|| format!("{}_mat_default", prefix));

            let material_name = if let Some(skel_name) = &node_skeleton_name {
                let skinned_mat = format!("{}__skinned", raw_material);
                if !renderer.has_material(&skinned_mat) {
                    let base = renderer
                        .materials
                        .get(&raw_material)
                        .cloned()
                        .unwrap_or_default();
                    renderer.add_material_with_skeleton(&skinned_mat, base, skel_name);
                }
                skinned_mat
            } else {
                raw_material
            };

            instances.push(GltfInstance {
                mesh_name,
                material_name,
                model: world,
                skeleton_name: node_skeleton_name.clone(),
                default_animation: default_animation_name.clone(),
                node_name: node.name().map(|s| s.to_string()),
            });
        }
    }

    // ============================================================
    // Шаг 7: анимации
    // ============================================================
    let animations = if opts.load_animations {
        load_animations(&doc, &buffers, &skeletons, prefix)?
    } else {
        HashMap::new()
    };

    log::info!(
        "glTF '{}': {} meshes, {} textures, {} materials, {} skeletons, {} animations, {} instances",
        path.display(),
        meshes_info.len(),
        texture_name_map.len(),
        material_name_map.len(),
        skeletons.len(),
        animations.len(),
        instances.len()
    );

    Ok(LoadedGltf {
        instances,
        skeletons,
        animations,
        meshes: meshes_info,
    })
}

// ============================================================
// Шаг 2: материалы
// ============================================================

fn load_materials(
    renderer: &mut Renderer,
    doc: &gltf::Document,
    prefix: &str,
    texture_name_map: &HashMap<usize, String>,
) -> Result<HashMap<usize, String>> {
    use crate::render::material::{AlphaMode, SamplerDesc, SamplerFilter, WrapMode};

    let mut map: HashMap<usize, String> = HashMap::new();

    for (i, mat) in doc.materials().enumerate() {
        let pbr = mat.pbr_metallic_roughness();
        let base = pbr.base_color_factor();

        let mut material = Material::new([base[0], base[1], base[2], base[3]]);
        material.metallic = pbr.metallic_factor();
        material.roughness = pbr.roughness_factor();

        if let Some(tex_info) = pbr.base_color_texture() {
            let img_idx = tex_info.texture().source().index();
            if let Some(name) = texture_name_map.get(&img_idx) {
                material.base_color_texture = Some(name.clone());
            }
        }

        if let Some(tex_info) = pbr.metallic_roughness_texture() {
            let img_idx = tex_info.texture().source().index();
            if let Some(name) = texture_name_map.get(&img_idx) {
                material.metallic_roughness_texture = Some(name.clone());
            }
        }

        if let Some(normal_info) = mat.normal_texture() {
            let img_idx = normal_info.texture().source().index();
            if let Some(name) = texture_name_map.get(&img_idx) {
                material.normal_texture = Some(name.clone());
                material.normal_scale = normal_info.scale();
            }
        }

        let em = mat.emissive_factor();
        material.emissive = [em[0], em[1], em[2]];
        if let Some(tex_info) = mat.emissive_texture() {
            let img_idx = tex_info.texture().source().index();
            if let Some(name) = texture_name_map.get(&img_idx) {
                material.emissive_texture = Some(name.clone());
            }
        }

        // --- alpha mode / cutoff ---
        material.alpha_mode = match mat.alpha_mode() {
            gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => AlphaMode::Mask,
            gltf::material::AlphaMode::Blend => AlphaMode::Blend,
        };
        material.alpha_cutoff = mat.alpha_cutoff().unwrap_or(0.5);

        // --- double-sided ---
        material.double_sided = mat.double_sided();

        // --- sampler из base_color_texture ---
        if let Some(tex_info) = pbr.base_color_texture() {
            let s = tex_info.texture().sampler();
            let wrap = |w: gltf::texture::WrappingMode| match w {
                gltf::texture::WrappingMode::Repeat => WrapMode::Repeat,
                gltf::texture::WrappingMode::ClampToEdge => WrapMode::ClampToEdge,
                gltf::texture::WrappingMode::MirroredRepeat => WrapMode::MirroredRepeat,
            };
            let flt_mag = |f: gltf::texture::MagFilter| match f {
                gltf::texture::MagFilter::Nearest => SamplerFilter::Nearest,
                gltf::texture::MagFilter::Linear => SamplerFilter::Linear,
            };
            let flt_min = |f: gltf::texture::MinFilter| match f {
                gltf::texture::MinFilter::Nearest
                | gltf::texture::MinFilter::NearestMipmapNearest
                | gltf::texture::MinFilter::NearestMipmapLinear => SamplerFilter::Nearest,
                _ => SamplerFilter::Linear,
            };
            material.sampler = SamplerDesc {
                wrap_u: wrap(s.wrap_s()),
                wrap_v: wrap(s.wrap_t()),
                mag_filter: flt_mag(
                    s.mag_filter()
                        .unwrap_or(gltf::texture::MagFilter::Linear),
                ),
                min_filter: flt_min(
                    s.min_filter()
                        .unwrap_or(gltf::texture::MinFilter::Linear),
                ),
                mip_filter: SamplerFilter::Linear,
            };
        }

        let name = format!("{}_mat_{}", prefix, i);
        renderer.add_material(&name, material);
        map.insert(i, name);
    }

    if map.is_empty() {
        let name = format!("{}_mat_default", prefix);
        renderer.add_material(&name, Material::default());
    }

    Ok(map)
}

// ============================================================
// Шаг 3: скелеты
// ============================================================

#[allow(clippy::type_complexity)]
fn load_skeletons(
    renderer: &mut Renderer,
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    prefix: &str,
    max_joints: usize,
) -> Result<(HashMap<String, Skeleton>, HashMap<usize, String>)> {
    let mut skeletons: HashMap<String, Skeleton> = HashMap::new();
    let mut by_index: HashMap<usize, String> = HashMap::new();

    for (skin_idx, skin) in doc.skins().enumerate() {
        let skeleton_name = format!("{}_skel_{}", prefix, skin_idx);

        let joints: Vec<_> = skin.joints().collect();
        if joints.len() > max_joints {
            log::warn!(
                "Skeleton '{}' has {} joints > max {} — truncating",
                skeleton_name,
                joints.len(),
                max_joints
            );
        }
        let joints = &joints[..joints.len().min(max_joints)];

        let mut node_to_joint: HashMap<usize, usize> = HashMap::new();
        let mut names = Vec::with_capacity(joints.len());
        let mut node_indices = Vec::with_capacity(joints.len());
        for (joint_idx, node) in joints.iter().enumerate() {
            let node_index = node.index();
            names.push(node.name().unwrap_or("bone").to_string());
            node_indices.push(node_index);
            node_to_joint.insert(node_index, joint_idx);
        }

        let mut parents = vec![PARENT_NONE; joints.len()];
        for node in doc.nodes() {
            let parent_idx = node.index();
            for child in node.children() {
                let child_idx = child.index();
                if let (Some(&pj), Some(&cj)) = (
                    node_to_joint.get(&parent_idx),
                    node_to_joint.get(&child_idx),
                ) {
                    parents[cj] = pj;
                }
            }
        }

        let inverse_bind: Vec<Mat4> = match skin.inverse_bind_matrices() {
            Some(accessor) => read_mat4_accessor(&accessor, buffers, joints.len()),
            None => vec![Mat4::IDENTITY; joints.len()],
        };

        let local_bind: Vec<Mat4> = joints
            .iter()
            .map(|n| Mat4::from_cols_array_2d(&n.transform().matrix()))
            .collect();

        let skel = Skeleton {
            names,
            node_indices,
            parents,
            inverse_bind,
            local_bind,
        };

        renderer.add_skeleton(&skeleton_name, &skel.bind_pose());
        skeletons.insert(skeleton_name.clone(), skel);
        by_index.insert(skin_idx, skeleton_name);
    }

    Ok((skeletons, by_index))
}

// ============================================================
// Шаг 4: меши
// ============================================================

fn upload_primitive_mesh(
    renderer: &mut Renderer,
    primitive: &gltf::Primitive,
    buffers: &[gltf::buffer::Data],
    name: &str,
) -> Result<usize> {
    let reader = primitive.reader(|b| Some(&buffers[b.index()]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .map(|iter| iter.collect())
        .unwrap_or_default();

    if positions.is_empty() {
        bail!("primitive '{}' has no positions", name);
    }

    let vertex_count = positions.len();

    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|iter| iter.collect())
        .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; vertex_count]);

    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|tc| tc.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0, 0.0]; vertex_count]);

    let indices: Vec<u32> = reader
        .read_indices()
        .map(|i| i.into_u32().collect())
        .unwrap_or_else(|| (0..vertex_count as u32).collect());

    let joints_u16: Vec<[u16; 4]> = reader
        .read_joints(0)
        .map(|iter| {
            iter.into_u16()
                .map(|j| [j[0], j[1], j[2], j[3]])
                .collect()
        })
        .unwrap_or_else(|| vec![[0, 0, 0, 0]; vertex_count]);

    let weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|iter| iter.into_f32().collect())
        .unwrap_or_else(|| vec![[0.0; 4]; vertex_count]);

    let mut vertices = Vec::with_capacity(vertex_count);
    for i in 0..vertex_count {
        let j = joints_u16.get(i).copied().unwrap_or([0, 0, 0, 0]);
        let mut w = weights.get(i).copied().unwrap_or([0.0; 4]);

        let sum = w[0] + w[1] + w[2] + w[3];
        if sum > 1e-6 && (sum - 1.0).abs() > 1e-4 {
            let inv = 1.0 / sum;
            w = [w[0] * inv, w[1] * inv, w[2] * inv, w[3] * inv];
        }

        vertices.push(Vertex3D {
            position: positions[i],
            normal: normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
            uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            color: [1.0, 1.0, 1.0, 1.0],
            joints: [j[0] as u32, j[1] as u32, j[2] as u32, j[3] as u32],
            weights: w,
        });
    }

    let mesh = Mesh::new(&renderer.device, &vertices, &indices, name);
    renderer.add_mesh(name, mesh);

    Ok(vertex_count)
}

// ============================================================
// Шаг 5: world-матрицы нод
// ============================================================

fn collect_world_transforms(doc: &gltf::Document) -> HashMap<usize, Mat4> {
    let mut out: HashMap<usize, Mat4> = HashMap::new();

    if let Some(scene) = doc.default_scene() {
        for node in scene.nodes() {
            walk_node(&node, Mat4::IDENTITY, &mut out);
        }
    } else {
        for scene in doc.scenes() {
            for node in scene.nodes() {
                walk_node(&node, Mat4::IDENTITY, &mut out);
            }
        }
    }

    out
}

fn walk_node(node: &gltf::Node, parent: Mat4, out: &mut HashMap<usize, Mat4>) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;
    out.insert(node.index(), world);

    for child in node.children() {
        walk_node(&child, world, out);
    }
}

// ============================================================
// Шаг 7: анимации
// ============================================================

/// Собирает `Track`, валидируя монотонность `times`.
/// Если timestamps не отсортированы (редкий, но допустимый glTF-паттерн),
/// сортируем пары (time, value) синхронно и логируем warning.
fn make_track(
    mut times: Vec<f32>,
    mut values: Vec<[f32; 4]>,
    anim_name: &str,
    node_idx: usize,
) -> Option<Track> {
    if times.is_empty() || values.is_empty() {
        return None;
    }
    // Усечение до min длины — некоторые экспортёры пишут лишние values.
    let n = times.len().min(values.len());
    times.truncate(n);
    values.truncate(n);

    // Проверка сортировки (строгое неубывание — дубликаты допустимы,
    // Track::sample обрабатывает dt ≈ 0).
    let sorted = times.windows(2).all(|w| w[0] <= w[1]);
    if !sorted {
        log::warn!(
            "Animation '{}' channel for node {}: times not monotonic — sorting {} keyframes",
            anim_name,
            node_idx,
            times.len()
        );
        let mut pairs: Vec<(f32, [f32; 4])> =
            times.into_iter().zip(values.into_iter()).collect();
        pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        times = pairs.iter().map(|p| p.0).collect();
        values = pairs.into_iter().map(|p| p.1).collect();
    }

    Some(Track { times, values })
}

fn load_animations(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    skeletons: &HashMap<String, Skeleton>,
    prefix: &str,
) -> Result<HashMap<String, AnimationClip>> {
    let mut animations: HashMap<String, AnimationClip> = HashMap::new();

    for (anim_idx, animation) in doc.animations().enumerate() {
        let name = animation
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{}_anim_{}", prefix, anim_idx));

        let mut target_skeleton: Option<&Skeleton> = None;
        for channel in animation.channels() {
            let node_idx = channel.target().node().index();
            for skel in skeletons.values() {
                if skel.node_indices.contains(&node_idx) {
                    target_skeleton = Some(skel);
                    break;
                }
            }
            if target_skeleton.is_some() {
                break;
            }
        }

        let Some(skel) = target_skeleton else {
            log::debug!(
                "Animation '{}' has no matching skeleton — skipping",
                name
            );
            continue;
        };

        let joint_count = skel.len();
        let mut clip = AnimationClip {
            name: name.clone(),
            duration: 0.0,
            translations: vec![None; joint_count],
            rotations: vec![None; joint_count],
            scales: vec![None; joint_count],
        };

        for channel in animation.channels() {
            let target = channel.target();
            let node_idx = target.node().index();

            let Some(&joint_idx) = skel.node_indices.iter().position(|&n| n == node_idx).as_ref()
            else {
                continue;
            };

            let reader = channel.reader(|b| Some(&buffers[b.index()]));
            let times: Vec<f32> = match reader.read_inputs() {
                Some(iter) => iter.collect(),
                None => continue,
            };
            if times.is_empty() {
                continue;
            }
            let last_time = *times.last().unwrap();
            if last_time > clip.duration {
                clip.duration = last_time;
            }

            let Some(outputs) = reader.read_outputs() else {
                continue;
            };

            match outputs {
                gltf::animation::util::ReadOutputs::Translations(iter) => {
                    let values: Vec<[f32; 4]> =
                        iter.map(|v| [v[0], v[1], v[2], 0.0]).collect();
                    if let Some(t) = make_track(times, values, &name, node_idx) {
                        clip.translations[joint_idx] = Some(t);
                    }
                }
                gltf::animation::util::ReadOutputs::Rotations(iter) => {
                    let values: Vec<[f32; 4]> = iter
                        .into_f32()
                        .map(|q| [q[0], q[1], q[2], q[3]])
                        .collect();
                    if let Some(t) = make_track(times, values, &name, node_idx) {
                        clip.rotations[joint_idx] = Some(t);
                    }
                }
                gltf::animation::util::ReadOutputs::Scales(iter) => {
                    let values: Vec<[f32; 4]> =
                        iter.map(|v| [v[0], v[1], v[2], 0.0]).collect();
                    if let Some(t) = make_track(times, values, &name, node_idx) {
                        clip.scales[joint_idx] = Some(t);
                    }
                }
                gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => {}
            }
        }

        animations.insert(name, clip);
    }

    Ok(animations)
}

// ============================================================
// Утилиты
// ============================================================

fn read_mat4_accessor(
    accessor: &gltf::Accessor,
    buffers: &[gltf::buffer::Data],
    fallback_count: usize,
) -> Vec<Mat4> {
    let count = accessor.count();
    let Some(view) = accessor.view() else {
        return vec![Mat4::IDENTITY; fallback_count];
    };
    let data = &buffers[view.buffer().index()];
    let base = view.offset() + accessor.offset();
    let stride = view.stride().unwrap_or(64);

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let offset = base + i * stride;
        if offset + 64 > data.len() {
            result.push(Mat4::IDENTITY);
            continue;
        }
        let vals: [f32; 16] = bytemuck::pod_read_unaligned(&data[offset..offset + 64]);
        result.push(Mat4::from_cols_array(&vals));
    }
    result
}