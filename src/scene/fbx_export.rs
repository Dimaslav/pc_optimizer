//! Экспорт сцены (или выделения) в FBX ASCII 7.4.
//!
//! Формат: 7.4, Y-up, X-right, единицы — метры (UnitScaleFactor = 1.0).
//! Топология: одна Geometry на entity, мировая матрица «запечена» в
//! вершины, Model имеет identity-трансформ. Иерархия Parent не
//! сохраняется — экспорт плоский. Материалы — phong (Diffuse/Specular/
//! Shininess/Opacity — маппинг из PBR-приближения).
//!
//! Открывается в Blender 3.x+, Maya, Unity, Unreal.

use anyhow::{bail, Context, Result};
use glam::{Mat3, Mat4, Vec3};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::ecs::{Entity, World};
use crate::game::components::{MaterialHandle, MeshHandle, Name, Transform, Visible};
use crate::render::material::Material;
use crate::render::mesh::Mesh;
use crate::render::Renderer;

// ============================================================
// Публичный API
// ============================================================

pub struct FbxExportOptions {
    /// `Some(list)` — экспортировать только эти entity.
    /// `None` — всю сцену.
    pub selected: Option<Vec<Entity>>,
}

impl Default for FbxExportOptions {
    fn default() -> Self {
        Self { selected: None }
    }
}

pub struct FbxExportStats {
    pub entities: usize,
    pub geometries: usize,
    pub materials: usize,
    pub total_vertices: usize,
    pub total_triangles: usize,
}

/// Экспортировать сцену в FBX-файл.
pub fn export_fbx(
    world: &World,
    renderer: &Renderer,
    path: impl AsRef<Path>,
    opts: &FbxExportOptions,
) -> Result<FbxExportStats> {
    let (text, stats) = build_fbx_string(world, renderer, opts)?;
    std::fs::write(path.as_ref(), text)
        .with_context(|| format!("write FBX {}", path.as_ref().display()))?;
    Ok(stats)
}

/// Только в память (без записи на диск) — для тестов/предпросмотра.
pub fn export_fbx_string(
    world: &World,
    renderer: &Renderer,
    opts: &FbxExportOptions,
) -> Result<(String, FbxExportStats)> {
    build_fbx_string(world, renderer, opts)
}

// ============================================================
// Сбор входных данных
// ============================================================

struct Item<'a> {
    /// Индекс исходной сущности (для логов).
    entity: Entity,
    /// Отображаемое имя.
    name: String,
    /// Мировая матрица (запекается в вершины).
    model: Mat4,
    mesh: &'a Mesh,
    material_name: Option<String>,
}

fn collect_items<'a>(
    world: &World,
    renderer: &'a Renderer,
    opts: &FbxExportOptions,
) -> Vec<Item<'a>> {
    let candidates: Vec<Entity> = match &opts.selected {
        Some(sel) => sel
            .iter()
            .copied()
            .filter(|e| world.entities().contains(e))
            .collect(),
        None => world.entities().to_vec(),
    };

    let mut items = Vec::new();
    for e in candidates {
        // Скрытые не экспортируем.
        if let Some(v) = world.get::<Visible>(e) {
            if !v.0 {
                continue;
            }
        }
        let Some(mh) = world.get::<MeshHandle>(e) else { continue };
        if world.get::<Transform>(e).is_none() {
            continue;
        }
        let Some(mesh) = renderer.meshes.get(&mh.0) else { continue };
        if mesh.cpu_vertices.is_empty() || mesh.cpu_indices.len() < 3 {
            continue;
        }

        let name = world
            .get::<Name>(e)
            .map(|n| n.0.clone())
            .unwrap_or_else(|| format!("Entity_{}", e));
        let material_name = world.get::<MaterialHandle>(e).map(|m| m.0.clone());
        let model = crate::game::world_matrix(world, e);

        items.push(Item {
            entity: e,
            name,
            model,
            mesh,
            material_name,
        });
    }

    items
}

// ============================================================
// Основной строитель
// ============================================================

fn build_fbx_string(
    world: &World,
    renderer: &Renderer,
    opts: &FbxExportOptions,
) -> Result<(String, FbxExportStats)> {
    let items = collect_items(world, renderer, opts);
    if items.is_empty() {
        bail!("nothing to export (no visible entities with Mesh + Transform)");
    }

    // Уникальные материалы → стабильные ID.
    let mut material_ids: HashMap<String, u64> = HashMap::new();
    let mut next_mat_id: u64 = 3_000_000;
    for it in &items {
        if let Some(mn) = &it.material_name {
            if !material_ids.contains_key(mn) {
                material_ids.insert(mn.clone(), next_mat_id);
                next_mat_id += 1;
            }
        }
    }

    // Geometry/Model IDs.
    let geom_base: u64 = 1_000_000;
    let model_base: u64 = 2_000_000;

    let mut out = String::with_capacity(1 << 20);

    write_header(&mut out);
    write_global_settings(&mut out);
    write_definitions(&mut out, items.len(), material_ids.len());

    writeln!(&mut out, "Objects:  {{").unwrap();

    let mut total_vertices = 0usize;
    let mut total_triangles = 0usize;

    for (i, it) in items.iter().enumerate() {
        let geom_id = geom_base + i as u64;
        let model_id = model_base + i as u64;

        let (v, t) = write_geometry(&mut out, geom_id, &it.name, it.mesh, &it.model);
        total_vertices += v;
        total_triangles += t;

        write_model(&mut out, model_id, &it.name);
    }

    for (name, &id) in &material_ids {
        let mat = renderer.materials.get(name).cloned().unwrap_or_default();
        write_material(&mut out, id, name, &mat);
    }

    writeln!(&mut out, "}}").unwrap();
    writeln!(&mut out).unwrap();

    write_connections(
        &mut out,
        &items,
        &material_ids,
        geom_base,
        model_base,
    );

    let stats = FbxExportStats {
        entities: items.len(),
        geometries: items.len(),
        materials: material_ids.len(),
        total_vertices,
        total_triangles,
    };

    Ok((out, stats))
}

// ============================================================
// Секции файла
// ============================================================

fn write_header(out: &mut String) {
    writeln!(out, "; FBX 7.4.0 project file").unwrap();
    writeln!(out, "; ----------------------------------------------------").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "FBXHeaderExtension:  {{").unwrap();
    writeln!(out, "    FBXHeaderVersion: 1003").unwrap();
    writeln!(out, "    FBXVersion: 7400").unwrap();
    writeln!(out, "    CreationTimeStamp:  {{").unwrap();
    writeln!(out, "        Version: 1000").unwrap();
    writeln!(out, "        Year: 2024").unwrap();
    writeln!(out, "        Month: 1").unwrap();
    writeln!(out, "        Day: 1").unwrap();
    writeln!(out, "        Hour: 0").unwrap();
    writeln!(out, "        Minute: 0").unwrap();
    writeln!(out, "        Second: 0").unwrap();
    writeln!(out, "        Millisecond: 0").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "    Creator: \"Rust Engine 3D FBX Exporter\"").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

fn write_global_settings(out: &mut String) {
    // Y-up, X-right, Z-forward-in. Единицы — метры.
    writeln!(out, "GlobalSettings:  {{").unwrap();
    writeln!(out, "    Version: 1000").unwrap();
    writeln!(out, "    Properties70:  {{").unwrap();
    writeln!(out, "        P: \"UpAxis\", \"int\", \"Integer\", \"\",1").unwrap();
    writeln!(out, "        P: \"UpAxisSign\", \"int\", \"Integer\", \"\",1").unwrap();
    writeln!(out, "        P: \"FrontAxis\", \"int\", \"Integer\", \"\",2").unwrap();
    writeln!(out, "        P: \"FrontAxisSign\", \"int\", \"Integer\", \"\",-1").unwrap();
    writeln!(out, "        P: \"CoordAxis\", \"int\", \"Integer\", \"\",0").unwrap();
    writeln!(out, "        P: \"CoordAxisSign\", \"int\", \"Integer\", \"\",1").unwrap();
    writeln!(out, "        P: \"UnitScaleFactor\", \"double\", \"Number\", \"\",100.0").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

fn write_definitions(out: &mut String, geom_count: usize, mat_count: usize) {
    writeln!(out, "Definitions:  {{").unwrap();
    writeln!(out, "    Version: 100").unwrap();
    writeln!(out, "    Count: 3").unwrap();
    writeln!(out, "    ObjectType: \"Model\" {{").unwrap();
    writeln!(out, "        Count: {}", geom_count).unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "    ObjectType: \"Geometry\" {{").unwrap();
    writeln!(out, "        Count: {}", geom_count).unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "    ObjectType: \"Material\" {{").unwrap();
    writeln!(out, "        Count: {}", mat_count).unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();
}

// ============================================================
// Geometry
// ============================================================

fn write_geometry(
    out: &mut String,
    id: u64,
    name: &str,
    mesh: &Mesh,
    model: &Mat4,
) -> (usize, usize) {
    // Нормальная матрица: inverse-transpose от 3x3-части мировой.
    let m3 = Mat3::from_mat4(*model);
    let nrm_mat = if m3.determinant().abs() > 1e-8 {
        m3.inverse().transpose()
    } else {
        Mat3::IDENTITY
    };

    // Мировые позиции/нормали + UV на каждую vertex.
    let n = mesh.cpu_vertices.len();
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(n);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(n);

    for v in &mesh.cpu_vertices {
        let p = model.transform_point3(Vec3::from(v.position));
        positions.push(p.to_array());

        let nw = (nrm_mat * Vec3::from(v.normal)).normalize_or_zero();
        // Если нормаль вырождена (zero), подставляем Y-up — чтобы
        // FBX-ридеры не показывали чёрные треугольники.
        let nw = if nw.length_squared() < 1e-8 {
            Vec3::Y
        } else {
            nw
        };
        normals.push(nw.to_array());

        uvs.push(v.uv);
    }

    // PolygonVertexIndex: последний индекс треугольника — отрицательный
    // (`-idx - 1`), так кодируется конец полигона в FBX.
    let tri_count = mesh.cpu_indices.len() / 3;
    let mut pvi: Vec<i64> = Vec::with_capacity(tri_count * 3);
    for tri in mesh.cpu_indices.chunks_exact(3) {
        pvi.push(tri[0] as i64);
        pvi.push(tri[1] as i64);
        pvi.push(-(tri[2] as i64) - 1);
    }

    // ByPolygonVertex / Direct: по одному значению на каждый
    // polygon-vertex, в том же порядке, что и PolygonVertexIndex.
    let mut poly_normals: Vec<[f32; 3]> = Vec::with_capacity(tri_count * 3);
    let mut poly_uvs: Vec<[f32; 2]> = Vec::with_capacity(tri_count * 3);
    for tri in mesh.cpu_indices.chunks_exact(3) {
        for k in 0..3 {
            let vi = tri[k] as usize;
            poly_normals.push(normals[vi]);
            poly_uvs.push(uvs[vi]);
        }
    }

    let safe_name = sanitize(name);

    writeln!(
        out,
        "    Geometry: {}, \"Geometry::{}\", \"Mesh\" {{",
        id, safe_name
    )
    .unwrap();

    // Vertices
    writeln!(out, "        Vertices: *{} {{", positions.len() * 3).unwrap();
    write!(out, "            a: ").unwrap();
    for (i, p) in positions.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "{:.6},{:.6},{:.6}", p[0], p[1], p[2]).unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "        }}").unwrap();

    // PolygonVertexIndex
    writeln!(out, "        PolygonVertexIndex: *{} {{", pvi.len()).unwrap();
    write!(out, "            a: ").unwrap();
    for (i, v) in pvi.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "{}", v).unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "        }}").unwrap();

    writeln!(out, "        GeometryVersion: 124").unwrap();

    // Normals
    writeln!(out, "        LayerElementNormal: 0 {{").unwrap();
    writeln!(out, "            Version: 101").unwrap();
    writeln!(out, "            Name: \"\"").unwrap();
    writeln!(out, "            MappingInformationType: \"ByPolygonVertex\"").unwrap();
    writeln!(out, "            ReferenceInformationType: \"Direct\"").unwrap();
    writeln!(out, "            Normals: *{} {{", poly_normals.len() * 3).unwrap();
    write!(out, "                a: ").unwrap();
    for (i, v) in poly_normals.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "{:.6},{:.6},{:.6}", v[0], v[1], v[2]).unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }}").unwrap();

    // UVs
    writeln!(out, "        LayerElementUV: 0 {{").unwrap();
    writeln!(out, "            Version: 101").unwrap();
    writeln!(out, "            Name: \"UVMap\"").unwrap();
    writeln!(out, "            MappingInformationType: \"ByPolygonVertex\"").unwrap();
    writeln!(out, "            ReferenceInformationType: \"Direct\"").unwrap();
    writeln!(out, "            UV: *{} {{", poly_uvs.len() * 2).unwrap();
    write!(out, "                a: ").unwrap();
    for (i, uv) in poly_uvs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "{:.6},{:.6}", uv[0], uv[1]).unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }}").unwrap();

    // Layer 0
    writeln!(out, "        Layer: 0 {{").unwrap();
    writeln!(out, "            Version: 100").unwrap();
    writeln!(out, "            LayerElement:  {{").unwrap();
    writeln!(out, "                Type: \"LayerElementNormal\"").unwrap();
    writeln!(out, "                TypedIndex: 0").unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "            LayerElement:  {{").unwrap();
    writeln!(out, "                Type: \"LayerElementUV\"").unwrap();
    writeln!(out, "                TypedIndex: 0").unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }}").unwrap();

    writeln!(out, "    }}").unwrap();

    (positions.len(), tri_count)
}

// ============================================================
// Model
// ============================================================

fn write_model(out: &mut String, id: u64, name: &str) {
    // Трансформ запечён в вершины — Model имеет identity.
    let safe = sanitize(name);
    writeln!(
        out,
        "    Model: {}, \"Model::{}\", \"Mesh\" {{",
        id, safe
    )
    .unwrap();
    writeln!(out, "        Version: 232").unwrap();
    writeln!(out, "        Properties70:  {{").unwrap();
    writeln!(out, "            P: \"Lcl Translation\", \"Lcl Translation\", \"\", \"A\",0,0,0").unwrap();
    writeln!(out, "            P: \"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\",0,0,0").unwrap();
    writeln!(out, "            P: \"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\",1,1,1").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "        Shading: T").unwrap();
    writeln!(out, "        Culling: \"CullingOff\"").unwrap();
    writeln!(out, "    }}").unwrap();
}

// ============================================================
// Material (PBR → phong approximation)
// ============================================================

fn write_material(out: &mut String, id: u64, name: &str, mat: &Material) {
    // FBX Material по умолчанию — phong. Конвертируем PBR → phong:
    //   - DiffuseColor = base_color.rgb
    //   - SpecularColor = mix(0.04, base_color.rgb, metallic)
    //   - Shininess = (1 - roughness) * 100
    //   - Opacity = base_color.a
    let br = mat.base_color[0];
    let bg = mat.base_color[1];
    let bb = mat.base_color[2];
    let ba = mat.base_color[3];

    let m = mat.metallic.clamp(0.0, 1.0);
    let r = mat.roughness.clamp(0.0, 1.0);

    let sr = 0.04 * (1.0 - m) + br * m;
    let sg = 0.04 * (1.0 - m) + bg * m;
    let sb = 0.04 * (1.0 - m) + bb * m;
    let shininess = (1.0 - r) * 100.0;

    // Emissive — FBX поддерживает через EmissiveColor.
    let er = mat.emissive[0];
    let eg = mat.emissive[1];
    let eb = mat.emissive[2];

    let safe = sanitize(name);
    writeln!(out, "    Material: {}, \"Material::{}\", \"\" {{", id, safe).unwrap();
    writeln!(out, "        Version: 102").unwrap();
    writeln!(out, "        ShadingModel: \"phong\"").unwrap();
    writeln!(out, "        MultiLayer: 0").unwrap();
    writeln!(out, "        Properties70:  {{").unwrap();
    writeln!(
        out,
        "            P: \"DiffuseColor\", \"Color\", \"\", \"A\",{:.6},{:.6},{:.6}",
        br, bg, bb
    )
    .unwrap();
    writeln!(
        out,
        "            P: \"SpecularColor\", \"Color\", \"\", \"A\",{:.6},{:.6},{:.6}",
        sr, sg, sb
    )
    .unwrap();
    writeln!(
        out,
        "            P: \"Shininess\", \"Number\", \"\", \"A\",{:.6}",
        shininess
    )
    .unwrap();
    writeln!(
        out,
        "            P: \"Opacity\", \"Number\", \"\", \"A\",{:.6}",
        ba
    )
    .unwrap();
    writeln!(
        out,
        "            P: \"EmissiveColor\", \"Color\", \"\", \"A\",{:.6},{:.6},{:.6}",
        er, eg, eb
    )
    .unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

// ============================================================
// Connections
// ============================================================

fn write_connections(
    out: &mut String,
    items: &[Item<'_>],
    material_ids: &HashMap<String, u64>,
    geom_base: u64,
    model_base: u64,
) {
    writeln!(out, "Connections:  {{").unwrap();

    for (i, it) in items.iter().enumerate() {
        let geom_id = geom_base + i as u64;
        let model_id = model_base + i as u64;

        // Model → Root
        writeln!(out, "    C: \"OO\", {}, 0", model_id).unwrap();
        // Geometry → Model
        writeln!(out, "    C: \"OO\", {}, {}", geom_id, model_id).unwrap();

        // Material → Model
        if let Some(mn) = &it.material_name {
            if let Some(&mid) = material_ids.get(mn) {
                writeln!(out, "    C: \"OO\", {}, {}", mid, model_id).unwrap();
            }
        }

        let _ = it; // silence unused
    }

    writeln!(out, "}}").unwrap();
}

// ============================================================
// Утилиты
// ============================================================

/// Убрать символы, ломающие FBX-строку в кавычках.
fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\0' => '_',
            _ => c,
        })
        .collect();
    if s.is_empty() {
        s.push_str("Unnamed");
    }
    s
}