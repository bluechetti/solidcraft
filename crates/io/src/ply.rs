//! PLY (polygon file format) import, from the format's public description: a text header naming
//! elements and their properties, then the elements' rows in ASCII or binary (little- or
//! big-endian). The vertices (`x`, `y`, `z` of any numeric type, and `red`, `green`, `blue` when
//! present) become a point cloud, and the faces (`vertex_indices`) a mesh. A raw range scan with
//! no faces but a `range_grid` (the scanner's image, one vertex or none per cell, as Stanford's
//! scans are written) is meshed from its grid. Every other element and property (normals,
//! intensity, edges, …) is passed over. The file is untrusted: counts are checked against its size
//! and capped.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::threemf::MeshObject;
use crate::{IoError, Result};

/// Largest header read (bytes).
const MAX_HEADER: usize = 1 << 20;
const MAX_ELEMENTS: usize = 64;
const MAX_PROPERTIES: usize = 256;
/// Most indices in one face.
const MAX_FACE: usize = 1024;
/// Most vertices and triangles of a mesh read from one file: as many as a mesh body holds.
const MAX_MESH_VERTICES: usize = solidcraft_kernel::MAX_VERTICES;
const MAX_TRIANGLES: usize = solidcraft_kernel::MAX_TRIANGLES;
/// Most cells of a range grid: each gives at most two triangles.
const MAX_GRID_CELLS: usize = MAX_TRIANGLES / 2;
/// A range grid's cell that saw nothing.
const EMPTY: u32 = u32::MAX;
/// Where a scan jumps from one surface to another behind it, neighbouring cells lie far apart: a
/// grid triangle whose longest edge is over this many times the grid's usual one is left out
/// rather than bridge the jump. On Stanford's passes those up to about 12 times lie on the merged
/// model; past 20, half hang in the air between surfaces.
const GAP: f64 = 8.0;

fn invalid(m: impl std::fmt::Display) -> IoError {
    IoError::Invalid(format!("PLY: {m}"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(s: &str) -> Option<Scalar> {
        Some(match s {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }
    pub(crate) fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
    /// The value at the start of `b`.
    fn read(self, b: &[u8], big: bool) -> Option<f64> {
        fn take<const N: usize>(b: &[u8], big: bool) -> Option<[u8; N]> {
            let mut a: [u8; N] = b.get(..N)?.try_into().ok()?;
            if big {
                a.reverse();
            }
            Some(a)
        }
        Some(match self {
            Scalar::I8 => f64::from(i8::from_le_bytes(take(b, big)?)),
            Scalar::U8 => f64::from(*b.first()?),
            Scalar::I16 => f64::from(i16::from_le_bytes(take(b, big)?)),
            Scalar::U16 => f64::from(u16::from_le_bytes(take(b, big)?)),
            Scalar::I32 => f64::from(i32::from_le_bytes(take(b, big)?)),
            Scalar::U32 => f64::from(u32::from_le_bytes(take(b, big)?)),
            Scalar::F32 => f64::from(f32::from_le_bytes(take(b, big)?)),
            Scalar::F64 => f64::from_le_bytes(take(b, big)?),
        })
    }
    /// A colour channel of this type as a byte: integers by their range, floats from 0..1.
    pub(crate) fn channel(self, v: f64) -> u8 {
        let v = match self {
            Scalar::F32 | Scalar::F64 => v * 255.0,
            Scalar::U16 | Scalar::I16 => v / 257.0,
            Scalar::U32 | Scalar::I32 => v / 16_843_009.0,
            Scalar::U8 | Scalar::I8 => v,
        };
        if v.is_finite() { v.round().clamp(0.0, 255.0) as u8 } else { 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Scalar(Scalar),
    /// The count's type and the items' type.
    List(Scalar, Scalar),
}

#[derive(Clone, Debug)]
pub(crate) struct Property {
    pub(crate) name: String,
    pub(crate) kind: Kind,
}

#[derive(Clone, Debug)]
pub(crate) struct Element {
    pub(crate) name: String,
    pub(crate) count: u64,
    pub(crate) properties: Vec<Property>,
}

impl Element {
    /// Bytes per row in a binary file, when every property is a scalar.
    fn row_size(&self) -> Option<usize> {
        self.properties.iter().try_fold(0usize, |n, p| match p.kind {
            Kind::Scalar(s) => n.checked_add(s.size()),
            Kind::List(..) => None,
        })
    }
    fn find(&self, names: &[&str]) -> Option<usize> {
        names.iter().find_map(|n| self.properties.iter().position(|p| p.name.eq_ignore_ascii_case(n) && matches!(p.kind, Kind::Scalar(_))))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Encoding {
    Ascii,
    Little,
    Big,
}

/// What the header says, and where the rows start.
#[derive(Debug)]
pub(crate) struct Header {
    pub(crate) encoding: Encoding,
    pub(crate) elements: Vec<Element>,
    pub(crate) body: usize,
    /// The text of the `comment` lines.
    pub(crate) comments: Vec<String>,
    /// A range grid's columns and rows (`obj_info num_cols` and `num_rows`).
    pub(crate) grid: [Option<u64>; 2],
}

/// Does `bytes` start like a PLY file?
pub fn is_ply(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ply") && bytes.get(3).is_some_and(|b| matches!(b, b'\n' | b'\r'))
}

pub(crate) fn header(bytes: &[u8]) -> Result<Header> {
    if !is_ply(bytes) {
        return Err(invalid("not a PLY file (it must start with `ply`)"));
    }
    let head = bytes.get(..bytes.len().min(MAX_HEADER)).unwrap_or(bytes);
    let end = head.windows(10).position(|w| w == b"end_header").ok_or_else(|| invalid("no `end_header` in the first megabyte"))?;
    // The rows start after the end of that line (`\n`, `\r\n` or a lone `\r`).
    let mut body = end + 10;
    while head.get(body).is_some_and(|b| *b == b' ' || *b == b'\t') {
        body += 1;
    }
    match (head.get(body), head.get(body + 1)) {
        (Some(b'\r'), Some(b'\n')) => body += 2,
        (Some(b'\n' | b'\r'), _) => body += 1,
        (None, _) => {}
        _ => return Err(invalid("`end_header` is not on a line of its own")),
    }
    let text = String::from_utf8_lossy(head.get(..end).unwrap_or_default());
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    let mut comments = Vec::new();
    let mut grid = [None; 2];
    for line in text.split(['\n', '\r']).skip(1) {
        if let Some(c) = line.strip_prefix("comment ") {
            comments.push(c.trim().to_string());
            continue;
        }
        let mut it = line.split_whitespace();
        match it.next() {
            Some("format") => {
                encoding = Some(match it.next() {
                    Some("ascii") => Encoding::Ascii,
                    Some("binary_little_endian") => Encoding::Little,
                    Some("binary_big_endian") => Encoding::Big,
                    other => return Err(invalid(format!("unknown format `{}`", other.unwrap_or("")))),
                });
            }
            Some("element") => {
                let name = it.next().ok_or_else(|| invalid("an element without a name"))?.to_string();
                let count = it.next().and_then(|c| c.parse::<u64>().ok()).ok_or_else(|| invalid(format!("element `{name}` has no count")))?;
                if elements.len() >= MAX_ELEMENTS {
                    return Err(invalid("too many elements"));
                }
                elements.push(Element { name, count, properties: Vec::new() });
            }
            Some("property") => {
                let e = elements.last_mut().ok_or_else(|| invalid("a property before any element"))?;
                if e.properties.len() >= MAX_PROPERTIES {
                    return Err(invalid("too many properties"));
                }
                let first = it.next().unwrap_or("");
                let kind = if first == "list" {
                    let count = it.next().and_then(Scalar::parse).ok_or_else(|| invalid("a list property without a count type"))?;
                    let item = it.next().and_then(Scalar::parse).ok_or_else(|| invalid("a list property without an item type"))?;
                    if matches!(count, Scalar::F32 | Scalar::F64) {
                        return Err(invalid("a list's count must be an integer"));
                    }
                    Kind::List(count, item)
                } else {
                    Kind::Scalar(Scalar::parse(first).ok_or_else(|| invalid(format!("unknown property type `{first}`")))?)
                };
                let name = it.next().ok_or_else(|| invalid("a property without a name"))?.to_string();
                e.properties.push(Property { name, kind });
            }
            Some("obj_info") => {
                let slot = match it.next() {
                    Some("num_cols") => grid.get_mut(0),
                    Some("num_rows") => grid.get_mut(1),
                    _ => None,
                };
                if let Some(slot) = slot {
                    *slot = it.next().and_then(|n| n.parse::<u64>().ok());
                }
            }
            // `comment`, other `obj_info` and blank lines.
            _ => {}
        }
    }
    let encoding = encoding.ok_or_else(|| invalid("no `format` line"))?;
    Ok(Header { encoding, elements, body, comments, grid })
}

/// A row's values, read one after another.
pub(crate) enum Rows<'a> {
    Binary { b: &'a [u8], pos: usize, big: bool },
    Ascii { tokens: std::slice::Split<'a, u8, fn(&u8) -> bool> },
}

fn space(b: &u8) -> bool {
    b.is_ascii_whitespace()
}

impl<'a> Rows<'a> {
    pub(crate) fn new(h: &Header, bytes: &'a [u8]) -> Rows<'a> {
        let b = bytes.get(h.body..).unwrap_or_default();
        match h.encoding {
            Encoding::Ascii => Rows::Ascii { tokens: b.split(space as fn(&u8) -> bool) },
            Encoding::Little => Rows::Binary { b, pos: 0, big: false },
            Encoding::Big => Rows::Binary { b, pos: 0, big: true },
        }
    }
    pub(crate) fn value(&mut self, t: Scalar) -> Result<f64> {
        match self {
            Rows::Binary { b, pos, big } => {
                let v = b.get(*pos..).and_then(|r| t.read(r, *big)).ok_or_else(|| invalid("the file ends early"))?;
                *pos += t.size();
                Ok(v)
            }
            Rows::Ascii { tokens } => loop {
                let tok = tokens.next().ok_or_else(|| invalid("the file ends early"))?;
                if tok.is_empty() {
                    continue;
                }
                let s = std::str::from_utf8(tok).map_err(|_| invalid("a value is not a number"))?;
                // Integers written as decimals (`255.0`) are taken too.
                return s.parse::<f64>().map_err(|_| invalid(format!("`{}` is not a number", s.chars().take(32).collect::<String>())));
            },
        }
    }
    /// Pass over `n` values of type `t`.
    fn skip(&mut self, t: Scalar, n: usize) -> Result<()> {
        match self {
            Rows::Binary { b, pos, .. } => {
                *pos = n
                    .checked_mul(t.size())
                    .and_then(|k| pos.checked_add(k))
                    .filter(|p| *p <= b.len())
                    .ok_or_else(|| invalid("the file ends early"))?;
                Ok(())
            }
            Rows::Ascii { .. } => {
                for _ in 0..n {
                    self.value(t)?;
                }
                Ok(())
            }
        }
    }
    pub(crate) fn count(&mut self, t: Scalar) -> Result<usize> {
        let n = self.value(t)?;
        if !(n >= 0.0 && n <= MAX_FACE as f64 && n.fract() == 0.0) {
            return Err(invalid("a list is longer than SolidCraft reads"));
        }
        Ok(n as usize)
    }
}

/// What to read from an element's rows.
struct Wanted {
    /// Property indices of x, y, z.
    xyz: Option<[usize; 3]>,
    /// Property indices of red, green, blue and their types.
    rgb: Option<[(usize, Scalar); 3]>,
    /// The face list's property index.
    faces: Option<usize>,
    /// A range grid's list of the vertex in each cell.
    grid: Option<usize>,
}

impl Wanted {
    fn of(e: &Element) -> Wanted {
        let scalar = |i: usize| match e.properties.get(i).map(|p| p.kind) {
            Some(Kind::Scalar(s)) => Some(s),
            _ => None,
        };
        let is_vertex = e.name == "vertex";
        let xyz = is_vertex.then(|| Some([e.find(&["x"])?, e.find(&["y"])?, e.find(&["z"])?])).flatten();
        let rgb = is_vertex
            .then(|| {
                let r = e.find(&["red", "r", "diffuse_red"])?;
                let g = e.find(&["green", "g", "diffuse_green"])?;
                let b = e.find(&["blue", "b", "diffuse_blue"])?;
                Some([(r, scalar(r)?), (g, scalar(g)?), (b, scalar(b)?)])
            })
            .flatten();
        let list =
            || e.properties.iter().position(|p| matches!(p.name.as_str(), "vertex_indices" | "vertex_index") && matches!(p.kind, Kind::List(..)));
        let faces = (e.name == "face").then(list).flatten();
        let grid = (e.name == "range_grid").then(list).flatten();
        Wanted { xyz, rgb, faces, grid }
    }
}

/// A vertex as read: its position (file units) and colour.
pub(crate) type Vertex = ([f64; 3], Option<[u8; 3]>);

/// A PLY file's vertices, rows `keep` says to keep, and their count in the file. Faces are read
/// into `faces` (a fan of triangles per face) and a range grid into `grid` (the vertex in each
/// cell, or [`EMPTY`]) when given.
struct Read {
    vertices: u64,
    bad_faces: usize,
    /// Grid cells naming a vertex that isn't there, or more than one.
    bad_cells: usize,
}

fn walk(
    bytes: &[u8],
    keep: &mut dyn FnMut(u64) -> bool,
    on_vertex: &mut dyn FnMut(Vertex),
    mut faces: Option<&mut Vec<[u32; 3]>>,
    mut grid: Option<&mut Vec<u32>>,
) -> Result<Read> {
    let h = header(bytes)?;
    let vertex = h.elements.iter().find(|e| e.name == "vertex").ok_or_else(|| invalid("no `vertex` element"))?;
    let w = Wanted::of(vertex);
    if w.xyz.is_none() {
        return Err(invalid("the vertices have no `x`, `y` and `z`"));
    }
    let mut out = Read { vertices: vertex.count, bad_faces: 0, bad_cells: 0 };
    let body = bytes.len().saturating_sub(h.body) as u64;
    let mut rows = Rows::new(&h, bytes);
    let mut slots = vec![0.0f64; MAX_PROPERTIES];
    let mut face: Vec<f64> = Vec::new();
    for e in &h.elements {
        let w = Wanted::of(e);
        let wanted_faces = w.faces.is_some() && faces.is_some();
        let wanted_grid = w.grid.is_some() && grid.is_some();
        let list = if wanted_faces {
            w.faces
        } else if wanted_grid {
            w.grid
        } else {
            None
        };
        // A binary element of scalars that isn't needed (or has rows to pass over) is a block
        // of bytes: check its size against the file before reading any of it.
        if let (Rows::Binary { b, pos, .. }, Some(size)) = (&mut rows, e.row_size()) {
            let total = e
                .count
                .checked_mul(size as u64)
                .filter(|n| *n <= (b.len() - *pos) as u64)
                .ok_or_else(|| invalid(format!("the file is too short for its {} {}", e.count, e.name)))?;
            if w.xyz.is_none() {
                *pos += total as usize;
                continue;
            }
            // The vertices: only the rows kept are read.
            let start = *pos;
            let big = matches!(h.encoding, Encoding::Big);
            let offsets: Vec<usize> = e
                .properties
                .iter()
                .scan(0usize, |o, p| {
                    let at = *o;
                    *o += match p.kind {
                        Kind::Scalar(s) => s.size(),
                        Kind::List(..) => 0,
                    };
                    Some(at)
                })
                .collect();
            let get = |row: &[u8], i: usize| -> Option<f64> {
                match e.properties.get(i)?.kind {
                    Kind::Scalar(s) => s.read(row.get(*offsets.get(i)?..)?, big),
                    Kind::List(..) => None,
                }
            };
            for r in 0..e.count {
                if !keep(r) {
                    continue;
                }
                let at = start + (r as usize) * size;
                let row = b.get(at..at + size).ok_or_else(|| invalid("the file ends early"))?;
                let Some([x, y, z]) = w.xyz else { break };
                let p = [get(row, x), get(row, y), get(row, z)];
                let [Some(x), Some(y), Some(z)] = p else { return Err(invalid("the file ends early")) };
                let rgb = match w.rgb.map(|c| c.map(|(i, t)| get(row, i).map(|v| t.channel(v)))) {
                    Some([Some(r), Some(g), Some(b)]) => Some([r, g, b]),
                    _ => None,
                };
                on_vertex(([x, y, z], rgb));
            }
            *pos += total as usize;
            // A point cloud needs nothing after its vertices.
            if faces.is_none() && grid.is_none() {
                break;
            }
            continue;
        }
        // Row by row: ASCII, or rows with lists. A row takes at least a byte, so a count larger
        // than the file is damage, not a long loop.
        if e.count > body {
            return Err(invalid(format!("the file is too short for its {} {}", e.count, e.name)));
        }
        for r in 0..e.count {
            for (i, p) in e.properties.iter().enumerate() {
                match p.kind {
                    Kind::Scalar(t) => {
                        let v = rows.value(t)?;
                        if let Some(s) = slots.get_mut(i) {
                            *s = v;
                        }
                    }
                    Kind::List(c, t) => {
                        let n = rows.count(c)?;
                        if list == Some(i) {
                            face.clear();
                            for _ in 0..n {
                                face.push(rows.value(t)?);
                            }
                        } else {
                            rows.skip(t, n)?;
                        }
                    }
                }
            }
            if let Some([x, y, z]) = w.xyz {
                if keep(r) {
                    let v = |i: usize| slots.get(i).copied().unwrap_or(0.0);
                    let rgb = w.rgb.map(|c| c.map(|(i, t)| t.channel(v(i))));
                    on_vertex(([v(x), v(y), v(z)], rgb));
                }
            } else if wanted_faces && let Some(tris) = faces.as_deref_mut() {
                let n = out.vertices;
                let idx: Option<Vec<u32>> =
                    face.iter().map(|v| (*v >= 0.0 && *v < n as f64 && v.fract() == 0.0).then(|| u32::try_from(*v as u64).ok()).flatten()).collect();
                match idx {
                    Some(p) if p.len() >= 3 => {
                        for k in 1..p.len() - 1 {
                            tris.push([p[0], p[k], p[k + 1]]);
                        }
                        if tris.len() > MAX_TRIANGLES {
                            return Err(invalid(format!("more than {MAX_TRIANGLES} triangles")));
                        }
                    }
                    _ => out.bad_faces += 1,
                }
            } else if wanted_grid && let Some(cells) = grid.as_deref_mut() {
                let n = out.vertices;
                let seen = face.first().filter(|v| **v >= 0.0 && **v < n as f64 && v.fract() == 0.0).and_then(|v| u32::try_from(*v as u64).ok());
                if face.len() > 1 || (seen.is_none() && !face.is_empty()) {
                    out.bad_cells += 1;
                }
                cells.push(seen.unwrap_or(EMPTY));
            }
        }
        if w.xyz.is_some() && faces.is_none() && grid.is_none() {
            break;
        }
    }
    Ok(out)
}

/// How many vertices and faces a PLY file says it has (from its header alone).
pub fn ply_counts(bytes: &[u8]) -> Result<(u64, u64)> {
    let h = header(bytes)?;
    let count = |name: &str| h.elements.iter().find(|e| e.name == name).map_or(0, |e| e.count);
    Ok((count("vertex"), count("face")))
}

/// A PLY file's faces as a mesh named `name` (a fan of triangles per face; colours, normals and
/// texture coordinates are not read), with what could not be read.
pub fn read_ply_mesh(bytes: &[u8], name: &str) -> Result<(Vec<MeshObject>, Vec<String>)> {
    let (vertices, faces) = ply_counts(bytes)?;
    if faces == 0 && has_range_grid(bytes) {
        return read_range_grid(bytes, name);
    }
    if faces == 0 {
        return Err(invalid("the file has no faces: it is a point cloud (insert it with pointcloud.insert)"));
    }
    if vertices > MAX_MESH_VERTICES as u64 {
        return Err(invalid(format!("more than {MAX_MESH_VERTICES} vertices")));
    }
    let mut positions: Vec<Vec3> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut finite = true;
    let r = walk(
        bytes,
        &mut |_| true,
        &mut |([x, y, z], _)| {
            finite &= x.is_finite() && y.is_finite() && z.is_finite();
            positions.push(Vec3::new(x, y, z));
        },
        Some(&mut triangles),
        None,
    )?;
    if !finite {
        return Err(invalid("a vertex is not a finite number"));
    }
    if triangles.is_empty() {
        return Err(invalid("no face could be read"));
    }
    let mut warnings = Vec::new();
    if r.bad_faces > 0 {
        warnings.push(format!("{} face(s) could not be read and were left out", r.bad_faces));
    }
    Ok((vec![MeshObject { name: name.to_string(), positions, triangles, ..Default::default() }], warnings))
}

/// Does the file hold a range grid (a raw scan's image of the vertices it saw)?
pub fn has_range_grid(bytes: &[u8]) -> bool {
    header(bytes).is_ok_and(|h| h.elements.iter().any(|e| e.name == "range_grid" && e.count > 0))
}

/// A raw range scan meshed from its grid: neighbouring cells that each saw a vertex are joined.
fn read_range_grid(bytes: &[u8], name: &str) -> Result<(Vec<MeshObject>, Vec<String>)> {
    let h = header(bytes)?;
    let cells = h.elements.iter().find(|e| e.name == "range_grid").map_or(0, |e| e.count);
    let [Some(cols), Some(rows)] = h.grid else {
        return Err(invalid("the range grid has no `obj_info num_cols` and `obj_info num_rows`"));
    };
    if cols.checked_mul(rows) != Some(cells) {
        return Err(invalid(format!("the range grid has {cells} cells, not num_cols x num_rows ({cols} x {rows})")));
    }
    if cells > MAX_GRID_CELLS as u64 {
        return Err(invalid(format!("a range grid of more than {MAX_GRID_CELLS} cells")));
    }
    let vertices = h.elements.iter().find(|e| e.name == "vertex").map_or(0, |e| e.count);
    if vertices > MAX_MESH_VERTICES as u64 {
        return Err(invalid(format!("more than {MAX_MESH_VERTICES} vertices")));
    }
    let mut positions: Vec<Vec3> = Vec::new();
    let mut grid: Vec<u32> = Vec::new();
    let mut finite = true;
    let r = walk(
        bytes,
        &mut |_| true,
        &mut |([x, y, z], _)| {
            finite &= x.is_finite() && y.is_finite() && z.is_finite();
            positions.push(Vec3::new(x, y, z));
        },
        None,
        Some(&mut grid),
    )?;
    if !finite {
        return Err(invalid("a vertex is not a finite number"));
    }
    if grid.len() as u64 != cells {
        return Err(invalid("the range grid's cells have no `vertex_indices`"));
    }
    let triangles = grid_triangles(&grid, cols as usize, &positions);
    if triangles.is_empty() {
        return Err(invalid("no square of the range grid has three corners that saw a vertex"));
    }
    let mut warnings = Vec::new();
    if r.bad_cells > 0 {
        warnings.push(format!("{} range grid cell(s) could not be read and were left empty", r.bad_cells));
    }
    Ok((vec![MeshObject { name: name.to_string(), positions, triangles, ..Default::default() }], warnings))
}

/// Triangles over a range grid `cols` cells across. Each square of four neighbouring cells that
/// all saw a vertex gives two triangles, split along its shorter diagonal, and one with a corner
/// missing gives one. They go round each square across, then down, so the surface faces the
/// scanner as Stanford's passes are laid out (checked against their merged models). A triangle
/// with an edge over [`GAP`] times the grid's usual longest edge is left out.
fn grid_triangles(grid: &[u32], cols: usize, positions: &[Vec3]) -> Vec<[u32; 3]> {
    let at = |i: usize| grid.get(i).copied().filter(|v| *v != EMPTY && (*v as usize) < positions.len());
    let p = |v: u32| positions.get(v as usize).copied().unwrap_or_default();
    let rows = grid.len().checked_div(cols).unwrap_or(0);
    let mut out: Vec<[u32; 3]> = Vec::new();
    for r in 0..rows.saturating_sub(1) {
        for c in 0..cols.saturating_sub(1) {
            let i = r * cols + c;
            // Round the square: this cell, across, across and down, down.
            let ring: Vec<u32> = [at(i), at(i + 1), at(i + cols + 1), at(i + cols)].into_iter().flatten().collect();
            match ring.as_slice() {
                [a, b, e, d] => {
                    if (p(*a) - p(*e)).len2() <= (p(*b) - p(*d)).len2() {
                        out.extend([[*a, *b, *e], [*a, *e, *d]]);
                    } else {
                        out.extend([[*a, *b, *d], [*b, *e, *d]]);
                    }
                }
                [a, b, c] => out.push([*a, *b, *c]),
                _ => {}
            }
        }
    }
    // The longest edge of each, squared, and the usual one.
    let longest: Vec<f64> = out
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(p);
            (a - b).len2().max((b - c).len2()).max((c - a).len2())
        })
        .collect();
    let mut sorted = longest.clone();
    if sorted.is_empty() {
        return out;
    }
    let mid = sorted.len() / 2;
    let limit = *sorted.select_nth_unstable_by(mid, f64::total_cmp).1 * GAP * GAP;
    out.into_iter().zip(longest).filter(|(_, l)| *l <= limit).map(|(t, _)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vertices kept from at most `max` of them, how many the file has, and whether it has colours.
    fn vertices(bytes: &[u8], max: usize) -> Result<(Vec<Vertex>, u64, bool)> {
        let r = crate::ply_complex::read_ply_cloud(bytes, max)?;
        let colors = r.colors.as_ref();
        let out = r.positions.iter().enumerate().map(|(i, p)| (*p, colors.and_then(|c| c.get(i).copied()))).collect();
        Ok((out, r.vertices, colors.is_some()))
    }

    fn binary(big: bool) -> Vec<u8> {
        let order = if big { "big" } else { "little" };
        let mut b = format!(
            "ply\r\nformat binary_{order}_endian 1.0\r\ncomment made by hand\r\nelement camera 1\r\nproperty double k\r\nelement vertex 3\r\nproperty float32 x\r\nproperty float32 y\r\nproperty float32 z\r\nproperty uchar red\r\nproperty uchar green\r\nproperty uchar blue\r\nproperty float intensity\r\nelement face 1\r\nproperty list uchar int vertex_indices\r\nend_header\r\n"
        )
        .into_bytes();
        let f = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        b.extend(if big { 9.5f64.to_be_bytes() } else { 9.5f64.to_le_bytes() });
        for (i, p) in [[0.0f32, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.5, -3.0]].iter().enumerate() {
            for v in p {
                b.extend(f(*v));
            }
            b.extend([10 * i as u8, 20, 255]);
            b.extend(f(0.5));
        }
        b.push(3);
        for i in [0i32, 1, 2] {
            b.extend(if big { i.to_be_bytes() } else { i.to_le_bytes() });
        }
        b
    }

    #[test]
    fn binary_little_and_big_endian_with_colours_and_other_elements() {
        for big in [false, true] {
            let (v, n, colored) = vertices(&binary(big), 3).unwrap();
            assert_eq!((n, colored), (3, true));
            assert_eq!(v[1], ([1.5, 0.0, 0.0], Some([10, 20, 255])));
            assert_eq!(v[2].0, [0.0, 2.5, -3.0]);
            // A thinned file leaves its face out: it would lose a corner.
            let thin = crate::ply_complex::read_ply_cloud(&binary(big), 2).unwrap();
            assert_eq!((thin.positions.len(), thin.complex.triangle_count(), thin.cells_left_out), (2, 0, 1));
            let (mesh, warnings) = read_ply_mesh(&binary(big), "Tri").unwrap();
            assert!(warnings.is_empty());
            assert_eq!(mesh[0].triangles, [[0, 1, 2]]);
            assert_eq!(ply_counts(&binary(big)).unwrap(), (3, 1));
        }
    }

    #[test]
    fn ascii_with_float_colours_lists_and_a_quad() {
        let src = "ply\nformat ascii 1.0\nelement vertex 4\nproperty double x\nproperty double y\nproperty double z\nproperty float red\nproperty float green\nproperty float blue\n\
                   element face 1\nproperty list uchar uint vertex_index\nend_header\n\
                   0 0 0 1 0 0\n1 0 0 0 1 0\n1 1 0 0 0 1\n0 1 0 0.5 0.5 0.5\n4 0 1 2 3\n";
        let (v, n, colored) = vertices(src.as_bytes(), 4).unwrap();
        assert_eq!((n, colored, v.len()), (4, true, 4));
        assert_eq!(v[0].1, Some([255, 0, 0]));
        assert_eq!(v[3], ([0.0, 1.0, 0.0], Some([128, 128, 128])));
        let (mesh, _) = read_ply_mesh(src.as_bytes(), "Quad").unwrap();
        assert_eq!(mesh[0].triangles, [[0, 1, 2], [0, 2, 3]]);
        // Only points (as the glim maps are written): no colours, no faces.
        let points =
            "ply\nformat ascii 1.0\nelement vertex 2\nproperty float32 x\nproperty float32 y\nproperty float32 z\nend_header\n1 2 3\n4 5 6\n";
        let (v, n, colored) = vertices(points.as_bytes(), 2).unwrap();
        assert_eq!((v.len(), n, colored), (2, 2, false));
        assert_eq!(v[1], ([4.0, 5.0, 6.0], None));
        // Points alone are thinned: every second one.
        let (v, _, _) = vertices(points.as_bytes(), 1).unwrap();
        assert_eq!(v, [([1.0, 2.0, 3.0], None)]);
        assert!(read_ply_mesh(points.as_bytes(), "x").is_err(), "a point cloud is not a mesh");
    }

    /// A scan as large as Stanford's Lucy (14M vertices, 28M triangles) is not refused for its
    /// counts, only a file claiming more than a mesh body holds; a header claiming what the file
    /// cannot hold allocates nothing for it.
    #[test]
    fn a_28_million_triangle_scan_is_not_refused_for_its_size() {
        let header = |vertices: u64, faces: u64| {
            format!(
                "ply\nformat binary_little_endian 1.0\nelement vertex {vertices}\nproperty float x\nproperty float y\nproperty float z\n\
                 element face {faces}\nproperty list uchar int vertex_indices\nend_header\n"
            )
        };
        let lucy = read_ply_mesh(header(14_027_872, 28_055_742).as_bytes(), "Lucy").unwrap_err().to_string();
        assert!(!lucy.contains("more than"), "{lucy}");
        let over = solidcraft_kernel::MAX_VERTICES as u64 + 1;
        let refused = read_ply_mesh(header(over, 1).as_bytes(), "Huge").unwrap_err().to_string();
        assert!(refused.contains(&format!("more than {} vertices", solidcraft_kernel::MAX_VERTICES)), "{refused}");
    }

    /// A raw scan as Stanford writes them: `cols` x `rows` cells, a cell at column c and row r
    /// seeing (c, -r, z) with z from `depth`, or nothing where `depth` gives `None`.
    fn range_scan(cols: usize, rows: usize, depth: impl Fn(usize, usize) -> Option<f64>) -> String {
        let mut vertices = Vec::new();
        let mut cells = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                match depth(c, r) {
                    Some(z) => {
                        cells.push(format!("1 {}", vertices.len()));
                        vertices.push(format!("{c} {} {z}", -(r as f64)));
                    }
                    None => cells.push("0".into()),
                }
            }
        }
        format!(
            "ply\nformat ascii 1.0\nobj_info is_cyberware_data 1\nobj_info num_cols {cols}\nobj_info num_rows {rows}\nelement vertex {}\n\
             property float x\nproperty float y\nproperty float z\nelement range_grid {}\nproperty list uchar int vertex_indices\nend_header\n{}\n{}\n",
            vertices.len(),
            cells.len(),
            vertices.join("\n"),
            cells.join("\n")
        )
    }

    /// The triangles' normals, not normalized.
    fn normals(m: &MeshObject) -> Vec<Vec3> {
        m.triangles
            .iter()
            .map(|t| (m.positions[t[1] as usize] - m.positions[t[0] as usize]).cross(m.positions[t[2] as usize] - m.positions[t[0] as usize]))
            .collect()
    }

    #[test]
    fn a_range_grid_is_meshed_square_by_square_and_not_across_a_jump() {
        // A flat 4 x 3 grid: 3 x 2 squares, two triangles each, all wound the same way round
        // (across, then down: with rows running down -y that faces -z).
        let (mesh, warnings) = read_ply_mesh(range_scan(4, 3, |_, _| Some(0.0)).as_bytes(), "Pass").unwrap();
        assert!(warnings.is_empty());
        assert_eq!(mesh[0].triangles.len(), 12);
        assert!(normals(&mesh[0]).iter().all(|n| (n.z + 1.0).abs() < 1e-12 && n.x.abs() < 1e-12 && n.y.abs() < 1e-12), "{:?}", normals(&mesh[0]));
        // A cell that saw nothing leaves its square one triangle, the same way round.
        let (mesh, _) = read_ply_mesh(range_scan(4, 3, |c, r| ((c, r) != (0, 0)).then_some(0.0)).as_bytes(), "Pass").unwrap();
        assert_eq!(mesh[0].triangles.len(), 11);
        assert!(normals(&mesh[0]).iter().all(|n| n.z < 0.0));
        // A square is split along its shorter diagonal.
        let (mesh, _) = read_ply_mesh(range_scan(2, 2, |c, r| Some(if (c, r) == (1, 1) { 0.9 } else { 0.0 })).as_bytes(), "Pass").unwrap();
        let p = &mesh[0].positions;
        let shared: Vec<u32> = mesh[0].triangles[0].iter().copied().filter(|v| mesh[0].triangles[1].contains(v)).collect();
        assert_eq!(shared.len(), 2);
        let diagonal = (p[shared[0] as usize] - p[shared[1] as usize]).len();
        assert!((diagonal - 2f64.sqrt()).abs() < 1e-9, "the 0-0 to 1-1 diagonal is longer than the other: {diagonal}");
        // The middle of a 5 x 5 grid seen far behind the rest (the scan jumped to another
        // surface): each square round it is split along the diagonal that misses it, and the
        // triangle left touching it would bridge the jump, so it is left out.
        let (mesh, _) = read_ply_mesh(range_scan(5, 5, |c, r| Some(if (c, r) == (2, 2) { -100.0 } else { 0.0 })).as_bytes(), "Pass").unwrap();
        assert_eq!(mesh[0].triangles.len(), 32 - 4);
        assert!(mesh[0].triangles.iter().all(|t| t.iter().all(|v| mesh[0].positions[*v as usize].z == 0.0)));
        // A cell naming a vertex that isn't there is left empty, with a warning.
        let bad = range_scan(3, 2, |_, _| Some(0.0)).replace("\n1 5\n", "\n1 99\n");
        let (mesh, warnings) = read_ply_mesh(bad.as_bytes(), "Pass").unwrap();
        assert_eq!((mesh[0].triangles.len(), warnings.len()), (3, 1), "{warnings:?}");
        // Without its size, or with one that doesn't match its cells, the grid can't be read.
        let flat = range_scan(3, 2, |_, _| Some(0.0));
        for broken in [
            flat.replace("obj_info num_cols 3\n", ""),
            flat.replace("num_rows 2", "num_rows 3"),
            flat.replace("num_cols 3", "num_cols 18446744073709551615"),
        ] {
            assert!(read_ply_mesh(broken.as_bytes(), "Pass").is_err(), "{broken}");
        }
        // Nothing to join: one cell saw a vertex.
        assert!(read_ply_mesh(range_scan(3, 2, |c, r| ((c, r) == (1, 1)).then_some(0.0)).as_bytes(), "Pass").is_err());
        assert!(has_range_grid(flat.as_bytes()));
        assert!(!has_range_grid(binary(false).as_slice()));
    }

    #[test]
    fn hostile_ply_never_panics() {
        let good = binary(false);
        let mut cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"ply".to_vec(),
            b"ply\nformat ascii 1.0\n".to_vec(),
            b"ply\nformat ascii 1.0\nelement vertex 18446744073709551615\nproperty float x\nproperty float y\nproperty float z\nend_header\n1 2 3".to_vec(),
            b"ply\nformat binary_little_endian 1.0\nelement vertex 18446744073709551615\nproperty float x\nproperty float y\nproperty float z\nend_header\n1234".to_vec(),
            b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty list uchar float x\nproperty float y\nproperty float z\nend_header\n\xff".to_vec(),
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n1".to_vec(),
            b"ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 1 0 0 0 1 0 3 0 1 99\n".to_vec(),
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\nnan inf -inf\n".to_vec(),
            b"ply\nformat ascii 1.0\nproperty float x\nend_header\n".to_vec(),
            b"ply\nformat binary_big_endian 1.0\nelement face 2\nproperty list float int vertex_indices\nend_header\n".to_vec(),
        ];
        // Every truncation of a good file, and of a range scan.
        for n in 0..good.len() {
            cases.push(good[..n].to_vec());
        }
        let scan = range_scan(3, 2, |c, _| (c != 1).then_some(0.0)).into_bytes();
        for n in 0..scan.len() {
            cases.push(scan[..n].to_vec());
        }
        cases.push(b"ply\nformat ascii 1.0\nobj_info num_cols 0\nobj_info num_rows 0\nelement vertex 0\nproperty float x\nproperty float y\nproperty float z\nelement range_grid 0\nproperty list uchar int vertex_indices\nend_header\n".to_vec());
        cases.push(b"ply\nformat ascii 1.0\nobj_info num_cols 1\nobj_info num_rows 2\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nelement range_grid 2\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n3 0 0 0\n1 -1\n".to_vec());
        for c in &cases {
            let r = std::panic::catch_unwind(|| {
                let _ = vertices(c, 1);
                let _ = vertices(c, 1000);
                let _ = read_ply_mesh(c, "x");
                let _ = ply_counts(c);
                let _ = has_range_grid(c);
            });
            assert!(r.is_ok(), "{:?}", String::from_utf8_lossy(c));
        }
        assert!(vertices(&good[..good.len() - 30], 1000).is_err(), "a cut-off file is refused");
        // A face naming a vertex that isn't there is left out, with a warning.
        let tri = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 2\nproperty list uchar int vertex_indices\nend_header\n0 0 0 1 0 0 0 1 0\n3 0 1 2\n3 0 1 99\n";
        let (mesh, warnings) = read_ply_mesh(tri.as_bytes(), "x").unwrap();
        assert_eq!((mesh[0].triangles.len(), warnings.len()), (1, 1));
    }
}
