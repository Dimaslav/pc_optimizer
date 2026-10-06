//! Парсер binary FBX 7.x (7000..7700).
//!
//! Возвращает тот же AST (`FbxNode`), что и `fbx_ast::parse_fbx_ascii`.
//!
//! Формат (reverse-engineered, см. Blender io_scene_fbx, assimp):
//!   header: 21B magic + 2B reserved + 4B version (u32 LE)  = 27B
//!   node*:  (u32|u64) end_offset
//!           (u32|u64) num_properties
//!           (u32|u64) property_list_len
//!           u8        name_len
//!           char[name_len] name
//!           property[num_properties]
//!           node*     (дочерние, до end_offset)
//!   terminated нулевым-нодом (end_offset == 0).
//!
//! version >= 7500: 64-bit offsets in NODE header.
//! version <  7500: 32-bit offsets in NODE header.

use anyhow::{bail, Context, Result};
use std::io::Read;

use super::fbx_ast::{FbxArg, FbxNode};

const BIN_MAGIC_21: &[u8] = b"Kaydara FBX Binary  \x00";
const BIN_MAGIC_23: &[u8] = b"Kaydara FBX Binary  \x00\x1a\x00";

const MIN_VERSION: u32 = 7000;
const MAX_VERSION: u32 = 7700;

const MAX_NUM_PROPERTIES: u64 = 1_000_000;
const MAX_ARRAY_LEN: u32 = 200_000_000;
const MAX_NAME_LEN: u8 = 200;

/// Максимальная глубина вложенности нод. Реальные FBX редко превышают 10.
/// Защита от malicious / corrupted файлов, вызывающих stack overflow
/// при рекурсивном `read_node`.
const MAX_NODE_DEPTH: u32 = 64;

pub fn is_binary_fbx(bytes: &[u8]) -> bool {
    bytes.len() >= BIN_MAGIC_21.len() && &bytes[..BIN_MAGIC_21.len()] == BIN_MAGIC_21
}

pub fn parse_fbx_binary(bytes: &[u8]) -> Result<Vec<FbxNode>> {
    if !is_binary_fbx(bytes) {
        bail!("не binary FBX (magic не совпал)");
    }
    if bytes.len() < 27 {
        bail!("FBX binary: слишком короткий файл ({} байт)", bytes.len());
    }

    let head_len = bytes.len().min(32);
    let head_hex: String = bytes[..head_len]
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");
    log::debug!("FBX binary head[{}]: {}", head_len, head_hex);

    let full_magic_ok = bytes.len() >= BIN_MAGIC_23.len()
        && &bytes[..BIN_MAGIC_23.len()] == BIN_MAGIC_23;

    if !full_magic_ok {
        log::debug!(
            "FBX binary: полная 23-байтовая магия не совпала (байты 21..23 = {:02X} {:02X}). \
             Использую 21-байтовую.",
            bytes[21], bytes[22]
        );
    }

    let v23 = read_u32_at(bytes, 23);
    let v21 = read_u32_at(bytes, 21);

    let (version, header_end) = if (MIN_VERSION..=MAX_VERSION).contains(&v23) {
        (v23, 27usize)
    } else if (MIN_VERSION..=MAX_VERSION).contains(&v21) {
        log::debug!(
            "FBX binary: версия найдена на offset 21 (= {}), а не 23. Сдвигаю header на 25 байт.",
            v21
        );
        (v21, 25usize)
    } else {
        bail!(
            "FBX binary: не удалось определить версию. \
             offset 23 → {}, offset 21 → {} (ожидается {}-{})",
            v23, v21, MIN_VERSION, MAX_VERSION
        );
    };

    log::debug!(
        "FBX binary: version = {}, header_end = {} байт, file_size = {}",
        version,
        header_end,
        bytes.len()
    );

    let mut cur = Cursor::new(bytes, header_end);
    let mut nodes = Vec::new();
    let root_end = bytes.len();
    loop {
        match read_node(&mut cur, version, 0, root_end)? {
            Some(n) => nodes.push(n),
            None => break,
        }
    }

    log::debug!(
        "FBX binary: parsed top-level nodes = {} (names: {})",
        nodes.len(),
        nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ")
    );

    Ok(nodes)
}

fn read_u32_at(b: &[u8], off: usize) -> u32 {
    if off + 4 > b.len() {
        return 0;
    }
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

// ============================================================
// Cursor
// ============================================================

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    fn pos(&self) -> usize { self.pos }
    fn set_pos(&mut self, p: usize) { self.pos = p; }
    fn remaining(&self) -> usize { self.data.len().saturating_sub(self.pos) }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos.saturating_add(n) > self.data.len() {
            bail!(
                "FBX binary: unexpected EOF (need {} at offset {}, file size {})",
                n, self.pos, self.data.len()
            );
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn i16(&mut self) -> Result<i16> { Ok(self.u16()? as i16) }
    fn i32(&mut self) -> Result<i32> { Ok(self.u32()? as i32) }
    fn i64(&mut self) -> Result<i64> { Ok(self.u64()? as i64) }
    fn f32(&mut self) -> Result<f32> { Ok(f32::from_bits(self.u32()?)) }
    fn f64(&mut self) -> Result<f64> { Ok(f64::from_bits(self.u64()?)) }
}

// ============================================================
// Node reader
// ============================================================

fn read_node(
    cur: &mut Cursor<'_>,
    version: u32,
    depth: u32,
    parent_end: usize,
) -> Result<Option<FbxNode>> {
    // Защита от malicious / corrupted файлов с бесконечной вложенностью.
    if depth > MAX_NODE_DEPTH {
        bail!(
            "FBX binary: exceeded max node depth {} at offset {}",
            MAX_NODE_DEPTH,
            cur.pos()
        );
    }

    let file_size = cur.data.len();
    let node_start = cur.pos();

    let min_header = if version >= 7500 { 25 } else { 13 };
    if cur.remaining() < min_header {
        return Ok(None);
    }

    let (end_offset, num_properties, property_list_len, name_len) =
        read_node_header(cur, version)?;

    if end_offset == 0 {
        return Ok(None);
    }

    if end_offset > file_size as u64 {
        bail!(
            "FBX binary: node@{}: end_offset {} > file_size {}",
            node_start, end_offset, file_size
        );
    }
    if (end_offset as usize) > parent_end {
        bail!(
            "FBX binary: node@{}: end_offset {} выходит за parent_end {}",
            node_start, end_offset, parent_end
        );
    }
    if num_properties > MAX_NUM_PROPERTIES {
        bail!(
            "FBX binary: node@{}: num_properties = {} > {}",
            node_start, num_properties, MAX_NUM_PROPERTIES
        );
    }
    if property_list_len as usize > cur.remaining() {
        bail!(
            "FBX binary: node@{}: property_list_len {} > remaining {}",
            node_start, property_list_len, cur.remaining()
        );
    }
    if name_len > MAX_NAME_LEN {
        bail!(
            "FBX binary: node@{}: name_len = {} > {}",
            node_start, name_len, MAX_NAME_LEN
        );
    }

    let name_bytes = cur.take(name_len as usize)?;
    let name = String::from_utf8_lossy(name_bytes).to_string();

    let props_start = cur.pos();
    let props_end = props_start
        .checked_add(property_list_len as usize)
        .ok_or_else(|| anyhow::anyhow!(
            "FBX binary: node@{} '{}': property_list_len overflow",
            node_start, name
        ))?;

    if props_end > end_offset as usize {
        bail!(
            "FBX binary: node@{} '{}': props_end {} > end_offset {}",
            node_start, name, props_end, end_offset
        );
    }

    let mut args: Vec<FbxArg> = Vec::new();
    for i in 0..num_properties {
        if cur.pos() >= props_end {
            bail!(
                "FBX binary: node@{} '{}': property #{} начинается за property_list_len",
                node_start, name, i
            );
        }
        args.push(read_property(cur)?);
        if cur.pos() > props_end {
            bail!(
                "FBX binary: node@{} '{}': property #{} пересекла property_list_len",
                node_start, name, i
            );
        }
    }

    if cur.pos() != props_end {
        log::warn!(
            "FBX binary: node@{} '{}': properties consumed {} bytes, header says {}",
            node_start,
            name,
            cur.pos() - props_start,
            property_list_len
        );
        cur.set_pos(props_end);
    }

    log::debug!(
        "FBX binary: node@{} '{}': end={}, props={}, args_read={}, name_len={}",
        node_start, name, end_offset, num_properties, args.len(), name_len
    );

    let end = end_offset as usize;
    let mut children = Vec::new();
    while cur.pos() < end {
        if end - cur.pos() < min_header {
            break;
        }
        match read_node(cur, version, depth + 1, end)? {
            Some(c) => children.push(c),
            None => break,
        }
    }

    if cur.pos() < end {
        cur.set_pos(end);
    }

    Ok(Some(FbxNode { name, args, children }))
}

fn read_node_header(cur: &mut Cursor<'_>, version: u32) -> Result<(u64, u64, u64, u8)> {
    if version >= 7500 {
        let end = cur.u64()?;
        let np = cur.u64()?;
        let plen = cur.u64()?;
        let nl = cur.u8()?;
        Ok((end, np, plen, nl))
    } else {
        let end = cur.u32()? as u64;
        let np = cur.u32()? as u64;
        let plen = cur.u32()? as u64;
        let nl = cur.u8()?;
        Ok((end, np, plen, nl))
    }
}

// ============================================================
// Property reader
// ============================================================

fn read_property(cur: &mut Cursor<'_>) -> Result<FbxArg> {
    let tag_offset = cur.pos();
    let tag = cur.u8()?;
    match tag as char {
        'Y' => Ok(FbxArg::Int(cur.i16()? as i64)),
        'C' => Ok(FbxArg::Int(cur.u8()? as i64)),
        'I' => Ok(FbxArg::Int(cur.i32()? as i64)),
        'F' => Ok(FbxArg::Float(cur.f32()? as f64)),
        'D' => Ok(FbxArg::Float(cur.f64()?)),
        'L' => Ok(FbxArg::Int(cur.i64()?)),
        'S' => {
            let len = cur.u32()? as usize;
            if len > cur.remaining() {
                bail!(
                    "FBX binary: string@{}: len {} > remaining {}",
                    tag_offset, len, cur.remaining()
                );
            }
            let bytes = cur.take(len)?;
            Ok(FbxArg::Str(String::from_utf8_lossy(bytes).to_string()))
        }
        'R' => {
            let len = cur.u32()? as usize;
            if len > cur.remaining() {
                bail!(
                    "FBX binary: raw@{}: len {} > remaining {}",
                    tag_offset, len, cur.remaining()
                );
            }
            let bytes = cur.take(len)?;
            Ok(FbxArg::Raw(bytes.to_vec()))
        }
        'f' | 'd' | 'l' | 'i' | 'b' => read_array(cur, tag as char, tag_offset),
        _ => bail!(
            "FBX binary: unknown property tag 0x{:02X} ('{}') at offset {}",
            tag, tag as char, tag_offset
        ),
    }
}

// ============================================================
// Array reader
// ============================================================

fn read_array(
    cur: &mut Cursor<'_>,
    tag: char,
    tag_offset: usize,
) -> Result<FbxArg> {
    let array_len = cur.u32()?;
    let encoding = cur.u32()?;
    let stored_len = cur.u32()? as usize;

    if array_len > MAX_ARRAY_LEN {
        bail!(
            "FBX binary: array@{} tag='{}': array_len = {} > {}",
            tag_offset, tag, array_len, MAX_ARRAY_LEN
        );
    }

    if encoding > 1 {
        bail!(
            "FBX binary: array@{} tag='{}': unsupported encoding = {}",
            tag_offset, tag, encoding
        );
    }

    let elem_size: usize = match tag {
        'f' | 'i' => 4,
        'd' | 'l' => 8,
        'b' => 1,
        _ => unreachable!(),
    };

    let raw_len = (array_len as usize)
        .checked_mul(elem_size)
        .ok_or_else(|| anyhow::anyhow!(
            "FBX binary: array@{} tag='{}': raw_len overflow",
            tag_offset, tag
        ))?;

    let bytes: Vec<u8> = if encoding == 0 {
        if stored_len < raw_len {
            bail!(
                "FBX binary: raw array@{} tag='{}': stored_len {} < raw_len {}",
                tag_offset, tag, stored_len, raw_len
            );
        }
        if stored_len > cur.remaining() {
            bail!(
                "FBX binary: raw array@{} tag='{}': stored_len {} > remaining {}",
                tag_offset, tag, stored_len, cur.remaining()
            );
        }
        let stored = cur.take(stored_len)?;
        stored[..raw_len].to_vec()
    } else {
        if stored_len > cur.remaining() {
            bail!(
                "FBX binary: compressed array@{}: stored_len {} > remaining {}",
                tag_offset, stored_len, cur.remaining()
            );
        }
        let compressed = cur.take(stored_len)?;
        let mut out = Vec::new();
        if out.try_reserve(raw_len).is_err() {
            bail!(
                "FBX binary: array@{}: cannot reserve {} bytes",
                tag_offset, raw_len
            );
        }
        let mut dec = flate2::read::ZlibDecoder::new(compressed);
        dec.read_to_end(&mut out)
            .with_context(|| format!("FBX binary: zlib decode at {}", tag_offset))?;
        if out.len() != raw_len {
            bail!(
                "FBX binary: array@{} tag='{}': decompressed {} bytes, expected {}",
                tag_offset, tag, out.len(), raw_len
            );
        }
        out
    };

    match tag {
        'f' => {
            let mut v = Vec::with_capacity(array_len as usize);
            for c in bytes.chunks_exact(4) {
                v.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64);
            }
            Ok(FbxArg::FloatArray(v))
        }
        'd' => {
            let mut v = Vec::with_capacity(array_len as usize);
            for c in bytes.chunks_exact(8) {
                v.push(f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
            }
            Ok(FbxArg::FloatArray(v))
        }
        'l' => {
            let mut v = Vec::with_capacity(array_len as usize);
            for c in bytes.chunks_exact(8) {
                v.push(i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]));
            }
            Ok(FbxArg::IntArray(v))
        }
        'i' => {
            let mut v = Vec::with_capacity(array_len as usize);
            for c in bytes.chunks_exact(4) {
                v.push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as i64);
            }
            Ok(FbxArg::IntArray(v))
        }
        'b' => {
            let mut v = Vec::with_capacity(array_len as usize);
            for c in bytes.chunks_exact(1) {
                v.push(c[0]);
            }
            Ok(FbxArg::BoolArray(v))
        }
        _ => unreachable!(),
    }
}