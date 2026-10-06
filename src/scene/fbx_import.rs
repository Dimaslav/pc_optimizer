//! Импорт FBX: ASCII 7.x и binary 7.0–7.5.
//!
//! Поддерживает: Model, Geometry (с нормалями и UV в ByPolygonVertex),
//! Material (phong), Connections (Model→Geometry, Model→Material,
//! Model→Model как Parent).
//!
//! Не поддерживает: скелеты (Deformer/Cluster), анимации
//! (AnimationCurveNode), текстуры, morph targets.
//!
//! Соглашение о единицах: мировые координаты = `value * UnitScaleFactor / 100`.
//! То есть FBX-файл со `UnitScaleFactor: 100.0` и значением `1.0` даёт 1 метр.

use anyhow::{anyhow, bail, Context, Result};
use glam::{EulerRot, Quat, Vec3};
use std::collections::HashMap;
use std::path::Path;

use crate::ecs::{Entity, World};
use crate::game::components::{MaterialHandle, MeshHandle, Name, Parent, Transform};
use crate::render::material::Material;
use crate::render::mesh::{Mesh, Vertex3D};
use crate::render::Renderer;

use super::fbx_ast::{parse_fbx_ascii, FbxArg, FbxNode};

// ============================================================
// Публичный API
// ============================================================

#[derive(Debug, Clone)]
pub struct FbxImportOptions {
    /// Дополнительный множитель к позициям (поверх UnitScaleFactor).
    pub scale: f32,
    /// Префикс имени для mesh/material/entity.
    pub prefix: String,
}

impl Default for FbxImportOptions {
    fn default() -> Self {
        Self { scale: 1.0, prefix: String::new() }
    }
}

#[derive(Debug, Default, Clone)]
pub struct FbxImportStats {
    pub models: usize,
    pub meshes: usize,
    pub materials: usize,
    pub total_vertices: usize,
    pub total_triangles: usize,
}

// ============================================================
// Точка входа
// ============================================================

pub fn import_fbx(
    world: &mut World,
    renderer: &mut Renderer,
    path: impl AsRef<Path>,
    opts: &FbxImportOptions,
) -> Result<FbxImportStats> {
    let path = path.as_ref();

    let bytes = std::fs::read(path)
        .with_context(|| format!("read FBX {}", path.display()))?;

    if bytes.is_empty() {
        bail!("FBX '{}' пустой", path.display());
    }

    let nodes = if super::fbx_binary::is_binary_fbx(&bytes) {
        log::info!("FBX '{}' — binary, парсим binary-формат", path.display());
        super::fbx_binary::parse_fbx_binary(&bytes)
            .with_context(|| format!("parse binary FBX {}", path.display()))?
    } else {
        let text_bytes: &[u8] = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            &bytes[3..]
        } else {
            &bytes[..]
        };

        let text = match std::str::from_utf8(text_bytes) {
            Ok(s) => s.to_string(),
            Err(e) => {
                let preview_len = bytes.len().min(120);
                let preview: String = bytes[..preview_len]
                    .iter()
                    .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
                    .collect();
                bail!(
                    "FBX '{}' не является ни binary-FBX (magic не совпал), \
                     ни текстовым ASCII-FBX (UTF-8 decode failed at byte {}).\n\
                     Первые байты: {}",
                    path.display(),
                    e.valid_up_to(),
                    preview
                );
            }
        };

        if !text.contains("FBXVersion") {
            bail!(
                "FBX '{}' не похож на ASCII FBX (нет `FBXVersion`).",
                path.display()
            );
        }

        parse_fbx_ascii(&text)
            .with_context(|| format!("parse ASCII FBX {}", path.display()))?
    };

    import_fbx_ast(world, renderer, &nodes, path, opts)
}

// ============================================================
// Верхний уровень
// ============================================================

fn import_fbx_ast(
    world: &mut World,
    renderer: &mut Renderer,
    nodes: &[FbxNode],
    path: &Path,
    opts: &FbxImportOptions,
) -> Result<FbxImportStats> {
    let unit_scale = find_unit_scale_factor(nodes).unwrap_or(1.0) as f32;
    let world_scale = (unit_scale / 100.0) * opts.scale;

    let objects = nodes
        .iter()
        .find(|n| n.name == "Objects")
        .ok_or_else(|| anyhow!("no `Objects` section in FBX"))?;

    let connections = nodes.iter().find(|n| n.name == "Connections");
    let conn_map = build_connection_map(connections);

    let mut geometries: HashMap<i64, FbxGeometry> = HashMap::new();
    let mut models: Vec<FbxModel> = Vec::new();
    let mut materials: HashMap<i64, FbxMaterial> = HashMap::new();

    for child in &objects.children {
        match child.name.as_str() {
            "Geometry" => {
                if let Some(g) = parse_geometry(child) {
                    geometries.insert(g.id, g);
                }
            }
            "Model" => {
                if let Some(m) = parse_model(child) {
                    models.push(m);
                }
            }
            "Material" => {
                if let Some(m) = parse_material(child) {
                    materials.insert(m.id, m);
                }
            }
            _ => {}
        }
    }

    if models.is_empty() {
        bail!("FBX не содержит ни одного Model");
    }

    let mut stats = FbxImportStats::default();
    stats.models = models.len();

    let mut model_to_entity: HashMap<i64, Entity> = HashMap::new();
    for m in &models {
        let e = world.spawn();
        world.insert(e, Name(format!("{}{}", opts.prefix, m.name)));
        world.insert(e, Transform {
            position: m.translation * world_scale,
            rotation: euler_deg_to_quat(m.rotation_deg, m.rotation_order),
            scale: m.scaling,
        });
        model_to_entity.insert(m.id, e);
    }

    for m in &models {
        let Some(&entity) = model_to_entity.get(&m.id) else { continue };
        let kids = conn_map.children_of.get(&m.id);

        if let Some(gid) = kids
            .and_then(|ks| ks.iter().find(|id| geometries.contains_key(id)).copied())
        {
            let geom = &geometries[&gid];
            if let Some(mesh) = build_mesh_from_geometry(&renderer.device, geom, world_scale) {
                let base = format!("{}_mesh", m.name);
                let mesh_name = unique_name(&base, |n| renderer.meshes.contains_key(n));
                stats.total_vertices += mesh.cpu_vertices.len();
                stats.total_triangles += mesh.cpu_indices.len() / 3;
                renderer.add_mesh(mesh_name.clone(), mesh);
                world.insert(entity, MeshHandle(mesh_name));
                stats.meshes += 1;
            }
        }

        if let Some(mid) = kids
            .and_then(|ks| ks.iter().find(|id| materials.contains_key(id)).copied())
        {
            let fbx_mat = &materials[&mid];
            let base = format!("{}_mat", fbx_mat.name);
            let mat_name = unique_name(&base, |n| renderer.has_material(n));
            renderer.add_material(mat_name.clone(), fbx_mat.to_material());
            world.insert(entity, MaterialHandle(mat_name));
            stats.materials += 1;
        }
    }

    for m in &models {
        let Some(&entity) = model_to_entity.get(&m.id) else { continue };
        let parent_id = conn_map.parent_of.get(&m.id).copied();
        if let Some(pid) = parent_id {
            if pid != 0 {
                if let Some(&pe) = model_to_entity.get(&pid) {
                    if pe != entity {
                        world.insert(entity, Parent(pe));
                    }
                }
            }
        }
    }

    log::info!(
        "FBX imported: {} models, {} meshes, {} materials, \
         {} verts, {} tris (unit_scale={:.4}, world_scale={:.4}) from {}",
        stats.models,
        stats.meshes,
        stats.materials,
        stats.total_vertices,
        stats.total_triangles,
        unit_scale,
        world_scale,
        path.display()
    );

    Ok(stats)
}

// ============================================================
// Connections
// ============================================================

#[derive(Default)]
struct ConnectionMap {
    children_of: HashMap<i64, Vec<i64>>,
    parent_of: HashMap<i64, i64>,
}

fn build_connection_map(connections: Option<&FbxNode>) -> ConnectionMap {
    let mut map = ConnectionMap::default();
    let Some(conn) = connections else { return map };

    for c in conn.children_named("C") {
        let ids: Vec<i64> = c.args.iter()
            .filter_map(|a| match a {
                FbxArg::Int(i) => Some(*i),
                _ => None,
            })
            .collect();
        if ids.len() < 2 {
            continue;
        }
        let child = ids[0];
        let parent = ids[1];
        map.children_of.entry(parent).or_default().push(child);
        map.parent_of.insert(child, parent);
    }

    map
}

// ============================================================
// UnitScaleFactor
// ============================================================

fn find_unit_scale_factor(nodes: &[FbxNode]) -> Option<f64> {
    let global = nodes.iter().find(|n| n.name == "GlobalSettings")?;
    let props = global.child("Properties70")?;
    for p in props.children_named("P") {
        let name = p.first_str()?;
        if name == "UnitScaleFactor" {
            for a in p.args.iter().rev() {
                match a {
                    FbxArg::Float(f) => return Some(*f),
                    FbxArg::Int(i) => return Some(*i as f64),
                    _ => {}
                }
            }
        }
    }
    None
}

// ============================================================
// Geometry
// ============================================================

struct FbxGeometry {
    id: i64,
    name: String,
    vertices: Vec<f32>,
    polygon_vertex_indices: Vec<i64>,
    normals: Option<Vec<f32>>,
    uvs: Option<Vec<f32>>,
}

fn parse_geometry(node: &FbxNode) -> Option<FbxGeometry> {
    let id = node.first_int()?;
    let raw_name = node
        .args.iter()
        .find_map(|a| match a {
            FbxArg::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("Geometry_{}", id));
    let name = raw_name.split("::").last().unwrap_or(&raw_name).to_string();

    let vertices = find_array(node, "Vertices")?;
    let pvi = find_int_array(node, "PolygonVertexIndex")?;
    if vertices.len() < 3 || pvi.len() < 3 {
        return None;
    }

    let normals = find_layer_array(node, "LayerElementNormal", "Normals");
    let uvs = find_layer_array(node, "LayerElementUV", "UV");

    Some(FbxGeometry {
        id,
        name,
        vertices,
        polygon_vertex_indices: pvi,
        normals,
        uvs,
    })
}

fn find_array(node: &FbxNode, name: &str) -> Option<Vec<f32>> {
    node.child(name)?.get_array_f32()
}

fn find_int_array(node: &FbxNode, name: &str) -> Option<Vec<i64>> {
    node.child(name)?.get_array_i64()
}

fn find_layer_array(node: &FbxNode, layer: &str, elem: &str) -> Option<Vec<f32>> {
    node.child(layer)?.child(elem)?.get_array_f32()
}

// ============================================================
// Model
// ============================================================

/// Порядок применения поворотов в FBX.
///
/// FBX enum `RotationOrder`:
/// 0 = eEulerXYZ, 1 = eEulerXZY, 2 = eEulerYZX,
/// 3 = eEulerYXZ, 4 = eEulerZXY, 5 = eEulerZYX, 6 = eSphericXYZ.
///
/// Применение `Quat::from_euler` в glam соответствует «XYZ» =
/// сначала X, потом Y, потом Z (в исходной нотации FBX для eEulerXYZ).
#[derive(Debug, Clone, Copy)]
enum RotationOrder {
    XYZ,
    XZY,
    YZX,
    YXZ,
    ZXY,
    ZYX,
}

impl Default for RotationOrder {
    fn default() -> Self {
        RotationOrder::XYZ
    }
}

impl RotationOrder {
    fn from_enum_code(code: i32) -> Self {
        match code {
            0 => Self::XYZ,
            1 => Self::XZY,
            2 => Self::YZX,
            3 => Self::YXZ,
            4 => Self::ZXY,
            5 => Self::ZYX,
            _ => Self::XYZ,
        }
    }

    fn to_euler_rot(self) -> EulerRot {
        match self {
            Self::XYZ => EulerRot::XYZ,
            Self::XZY => EulerRot::XZY,
            Self::YZX => EulerRot::YZX,
            Self::YXZ => EulerRot::YXZ,
            Self::ZXY => EulerRot::ZXY,
            Self::ZYX => EulerRot::ZYX,
        }
    }
}

struct FbxModel {
    id: i64,
    name: String,
    translation: Vec3,
    rotation_deg: Vec3,
    scaling: Vec3,
    rotation_order: RotationOrder,
}

fn parse_model(node: &FbxNode) -> Option<FbxModel> {
    let id = node.first_int()?;
    let raw_name = node
        .args.iter()
        .find_map(|a| match a {
            FbxArg::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("Model_{}", id));
    let name = raw_name.split("::").last().unwrap_or(&raw_name).to_string();

    let mut translation = Vec3::ZERO;
    let mut rotation_deg = Vec3::ZERO;
    let mut scaling = Vec3::ONE;
    let mut rotation_order = RotationOrder::default();

    if let Some(props) = node.child("Properties70") {
        for p in props.children_named("P") {
            let Some(prop_name) = p.first_str() else { continue };
            let floats = p.floats();

            if prop_name == "RotationOrder" {
                if let Some(&v) = floats.last() {
                    rotation_order = RotationOrder::from_enum_code(v as i32);
                }
                continue;
            }

            if floats.len() < 3 {
                continue;
            }
            let s = floats.len() - 3;
            let v = Vec3::new(floats[s], floats[s + 1], floats[s + 2]);
            match prop_name {
                "Lcl Translation" => translation = v,
                "Lcl Rotation" => rotation_deg = v,
                "Lcl Scaling" => scaling = v,
                _ => {}
            }
        }
    }

    Some(FbxModel {
        id,
        name,
        translation,
        rotation_deg,
        scaling,
        rotation_order,
    })
}

/// Применяет углы Эйлера (в градусах) в заданном порядке.
fn euler_deg_to_quat(deg: Vec3, order: RotationOrder) -> Quat {
    Quat::from_euler(
        order.to_euler_rot(),
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

// ============================================================
// Material
// ============================================================

struct FbxMaterial {
    id: i64,
    name: String,
    diffuse: [f32; 4],
    specular: [f32; 3],
    shininess: f32,
    opacity: f32,
    emissive: [f32; 3],
    metallic: Option<f32>,
}

impl FbxMaterial {
    fn to_material(&self) -> Material {
        let mut m = Material::new(self.diffuse);
        m.emissive = self.emissive;
        m.roughness = (1.0 - self.shininess / 100.0).clamp(0.05, 1.0);
        m.metallic = self.metallic.unwrap_or_else(|| {
            let dx = (self.diffuse[0] - self.specular[0]).abs();
            let dy = (self.diffuse[1] - self.specular[1]).abs();
            let dz = (self.diffuse[2] - self.specular[2]).abs();
            if (dx + dy + dz) < 0.05 && self.diffuse[0] > 0.3 {
                1.0
            } else {
                0.0
            }
        });
        m.base_color[3] = self.opacity;
        m
    }
}

fn parse_material(node: &FbxNode) -> Option<FbxMaterial> {
    let id = node.first_int()?;
    let raw_name = node
        .args.iter()
        .find_map(|a| match a {
            FbxArg::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("Material_{}", id));
    let name = raw_name.split("::").last().unwrap_or(&raw_name).to_string();

    let mut m = FbxMaterial {
        id,
        name,
        diffuse: [0.8, 0.8, 0.8, 1.0],
        specular: [0.04, 0.04, 0.04],
        shininess: 20.0,
        opacity: 1.0,
        emissive: [0.0, 0.0, 0.0],
        metallic: None,
    };

    if let Some(props) = node.child("Properties70") {
        for p in props.children_named("P") {
            let Some(prop_name) = p.first_str() else { continue };
            let floats = p.floats();
            let last3 = || -> Option<[f32; 3]> {
                if floats.len() < 3 {
                    return None;
                }
                let s = floats.len() - 3;
                Some([floats[s], floats[s + 1], floats[s + 2]])
            };
            match prop_name {
                "DiffuseColor" => {
                    if let Some(v) = last3() {
                        m.diffuse[0] = v[0];
                        m.diffuse[1] = v[1];
                        m.diffuse[2] = v[2];
                    }
                }
                "SpecularColor" => {
                    if let Some(v) = last3() {
                        m.specular = v;
                    }
                }
                "EmissiveColor" => {
                    if let Some(v) = last3() {
                        m.emissive = v;
                    }
                }
                "Shininess" => {
                    if let Some(v) = floats.last() {
                        m.shininess = *v;
                    }
                }
                "Opacity" => {
                    if let Some(v) = floats.last() {
                        m.opacity = *v;
                    }
                }
                "TransparencyFactor" => {
                    if let Some(v) = floats.last() {
                        if (m.opacity - 1.0).abs() < 1e-6 {
                            m.opacity = 1.0 - *v;
                        }
                    }
                }
                "Metallic" => {
                    if let Some(v) = floats.last() {
                        m.metallic = Some(*v);
                    }
                }
                _ => {}
            }
        }
    }

    Some(m)
}

// ============================================================
// Mesh builder
// ============================================================

fn build_mesh_from_geometry(
    device: &wgpu::Device,
    geom: &FbxGeometry,
    scale: f32,
) -> Option<Mesh> {
    let verts = &geom.vertices;
    let pvi = &geom.polygon_vertex_indices;
    if verts.len() < 3 || pvi.len() < 3 {
        return None;
    }

    let mut polygons: Vec<Vec<u32>> = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    for &raw in pvi {
        if raw < 0 {
            let idx = (-raw - 1) as u32;
            current.push(idx);
            polygons.push(std::mem::take(&mut current));
        } else {
            current.push(raw as u32);
        }
    }
    if !current.is_empty() {
        polygons.push(current);
    }

    let mut out_vertices: Vec<Vertex3D> = Vec::new();
    let mut out_indices: Vec<u32> = Vec::new();
    let mut cache: HashMap<u32, u32> = HashMap::new();

    let mut bv: u32 = 0;
    for poly in &polygons {
        if poly.len() < 3 {
            bv += poly.len() as u32;
            continue;
        }

        let mut idxs: Vec<u32> = Vec::with_capacity(poly.len());
        for (k, &v_id) in poly.iter().enumerate() {
            let bv_i = bv + k as u32;
            let out_i = if let Some(&i) = cache.get(&bv_i) {
                i
            } else {
                let pos = get_pos(verts, v_id as usize, scale);
                let normal = geom
                    .normals
                    .as_ref()
                    .and_then(|n| get_vec3(n, bv_i as usize))
                    .unwrap_or([0.0, 1.0, 0.0]);
                let uv = geom
                    .uvs
                    .as_ref()
                    .and_then(|u| get_vec2(u, bv_i as usize))
                    .unwrap_or([0.0, 0.0]);
                out_vertices.push(Vertex3D {
                    position: pos,
                    normal,
                    uv,
                    color: [1.0; 4],
                    joints: [0; 4],
                    weights: [0.0; 4],
                });
                let i = (out_vertices.len() - 1) as u32;
                cache.insert(bv_i, i);
                i
            };
            idxs.push(out_i);
        }

        for k in 1..idxs.len() - 1 {
            out_indices.push(idxs[0]);
            out_indices.push(idxs[k]);
            out_indices.push(idxs[k + 1]);
        }

        bv += poly.len() as u32;
    }

    if out_vertices.is_empty() || out_indices.len() < 3 {
        return None;
    }

    Some(Mesh::new(device, &out_vertices, &out_indices, &geom.name))
}

fn get_pos(flat: &[f32], i: usize, scale: f32) -> [f32; 3] {
    let b = i * 3;
    if b + 2 >= flat.len() {
        return [0.0, 0.0, 0.0];
    }
    [flat[b] * scale, flat[b + 1] * scale, flat[b + 2] * scale]
}

fn get_vec3(flat: &[f32], i: usize) -> Option<[f32; 3]> {
    let b = i * 3;
    if b + 2 >= flat.len() {
        return None;
    }
    Some([flat[b], flat[b + 1], flat[b + 2]])
}

fn get_vec2(flat: &[f32], i: usize) -> Option<[f32; 2]> {
    let b = i * 2;
    if b + 1 >= flat.len() {
        return None;
    }
    Some([flat[b], flat[b + 1]])
}

// ============================================================
// Утилиты
// ============================================================

fn unique_name(base: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(base) {
        return base.to_string();
    }
    let mut i = 1u32;
    loop {
        let n = format!("{}_{}", base, i);
        if !exists(&n) {
            return n;
        }
        i += 1;
    }
}