//! Point cloud files from scanners, read for a design's point cloud beside PLY and XYZ
//! ([`crate::point_cloud`]):
//!
//! - **PCD** (the Point Cloud Library's): its header, then the points as text, packed records, or
//!   packed field by field and compressed with LZF. A point with a NaN coordinate (an organised
//!   cloud's missing return) is left out; a colour is the packed `rgb`/`rgba` field.
//! - **PTX** (a laser scanner's stations as text): per station its columns and rows, the scanner's
//!   place and axes, and a 4 × 4 matrix that registers it (applied to a point as a row,
//!   `(x y z 1) · M`, the last row the move), then its points `x y z intensity [r g b]`. A point at
//!   exactly 0 0 0 is a missing return.
//! - **E57** (ASTM E2807, through the `e57` crate): every scan's points, spherical ones turned
//!   Cartesian, each moved by its scan's pose; points without a valid coordinate left out.
//! - **LAS and LAZ** (the ASPRS format, through the `las` crate with its own LAZ decompression):
//!   every point scaled and offset as the header says; colours of 16 bits a channel brought to 8.
//!
//! What is read of a point is where it is and its colour when the file has one; intensities are
//! read past. A file is refused with the reason: a header that does not agree with itself, data
//! that ends early, a field of a type that is not a number.

use std::io::Cursor;

use crate::{IoError, Result};

/// The extensions read here, lower case.
pub const EXTENSIONS: [&str; 5] = ["pcd", "ptx", "e57", "las", "laz"];

/// Points and their colours as a file states them, in its own units.
#[derive(Debug, Default, PartialEq)]
pub struct Points {
    pub points: Vec<[f64; 3]>,
    /// One a point, or `None` when the file has no colours.
    pub colors: Option<Vec<[u8; 3]>>,
}

fn bad(why: impl Into<String>) -> IoError {
    IoError::Invalid(why.into())
}

/// Reads a file with extension `ext` (one of [`EXTENSIONS`], any case), at most `most` points
/// (more is refused, naming how many).
pub fn read(bytes: &[u8], ext: &str, most: usize) -> Result<Points> {
    let read = match ext.to_ascii_lowercase().as_str() {
        "pcd" => read_pcd(bytes, most)?,
        "ptx" => read_ptx(bytes, most)?,
        "e57" => read_e57(bytes, most)?,
        "las" | "laz" => read_las(bytes, most)?,
        other => return Err(bad(format!("`.{other}` is not a point cloud format read here"))),
    };
    if read.points.is_empty() {
        return Err(bad("the file has no points"));
    }
    Ok(read)
}

fn too_many(count: u64, most: usize) -> IoError {
    bad(format!("the file has {count} points, more than the {most} read"))
}

// ---- PCD ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PcdData {
    Ascii,
    Binary,
    Compressed,
}

#[derive(Clone, Debug)]
struct PcdField {
    name: String,
    size: usize,
    kind: u8,
    count: usize,
}

/// Bytes `at..at + N` of `bytes` as an array, or zeros past the end (never read: the data's
/// length is checked against the header first).
fn take<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(s) = at.checked_add(N).and_then(|end| bytes.get(at..end)) {
        out.copy_from_slice(s);
    }
    out
}

impl PcdField {
    /// The value at `at` in `bytes` (little-endian, as PCL writes).
    fn decode(&self, bytes: &[u8], at: usize) -> f64 {
        match (self.kind, self.size) {
            (b'F', 4) => f64::from(f32::from_le_bytes(take(bytes, at))),
            (b'F', 8) => f64::from_le_bytes(take(bytes, at)),
            (b'I', 1) => f64::from(i8::from_le_bytes(take(bytes, at))),
            (b'I', 2) => f64::from(i16::from_le_bytes(take(bytes, at))),
            (b'I', 4) => f64::from(i32::from_le_bytes(take(bytes, at))),
            (b'I', 8) => i64::from_le_bytes(take(bytes, at)) as f64,
            (b'U', 1) => f64::from(u8::from_le_bytes(take(bytes, at))),
            (b'U', 2) => f64::from(u16::from_le_bytes(take(bytes, at))),
            (b'U', 4) => f64::from(u32::from_le_bytes(take(bytes, at))),
            _ => u64::from_le_bytes(take(bytes, at)) as f64,
        }
    }
}

fn read_pcd(data: &[u8], most: usize) -> Result<Points> {
    let mut offset = 0;
    let mut fields: Vec<PcdField> = Vec::new();
    let (mut sizes, mut types, mut counts): (Vec<usize>, Vec<u8>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::new());
    let (mut width, mut height, mut points): (Option<usize>, usize, Option<usize>) = (None, 1, None);
    let mut format = None;
    while offset < data.len() {
        let rest = data.get(offset..).unwrap_or_default();
        let end = rest.iter().position(|&b| b == b'\n').map_or(data.len(), |p| offset + p);
        let line = String::from_utf8_lossy(data.get(offset..end).unwrap_or_default()).trim().to_owned();
        offset = (end + 1).min(data.len());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let numbers = |what: &str| -> Result<Vec<usize>> {
            words.iter().skip(1).map(|w| w.parse::<usize>().map_err(|_| bad(format!("the PCD {what} line is not whole numbers")))).collect()
        };
        match words.first().map(|w| w.to_ascii_uppercase()).as_deref() {
            Some("VERSION" | "VIEWPOINT") => {}
            Some("FIELDS" | "COLUMNS") => {
                fields = words.iter().skip(1).map(|n| PcdField { name: n.to_ascii_lowercase(), size: 0, kind: 0, count: 1 }).collect();
            }
            Some("SIZE") => sizes = numbers("SIZE")?,
            Some("TYPE") => types = words.iter().skip(1).map(|w| w.bytes().next().unwrap_or(0).to_ascii_uppercase()).collect(),
            Some("COUNT") => counts = numbers("COUNT")?,
            Some("WIDTH") => width = numbers("WIDTH")?.first().copied(),
            Some("HEIGHT") => height = numbers("HEIGHT")?.first().copied().unwrap_or(1),
            Some("POINTS") => points = numbers("POINTS")?.first().copied(),
            Some("DATA") => {
                format = Some(match words.get(1).map(|w| w.to_ascii_lowercase()).as_deref() {
                    Some("ascii") => PcdData::Ascii,
                    Some("binary") => PcdData::Binary,
                    Some("binary_compressed") => PcdData::Compressed,
                    other => return Err(bad(format!("unknown PCD data `{}`", other.unwrap_or("")))),
                });
                break;
            }
            other => return Err(bad(format!("unknown PCD header line `{}`", other.unwrap_or("")))),
        }
    }
    let Some(format) = format else { return Err(bad("the PCD header has no DATA line")) };
    if fields.is_empty() || sizes.len() != fields.len() || types.len() != fields.len() || !(counts.is_empty() || counts.len() == fields.len()) {
        return Err(bad("the PCD header's FIELDS, SIZE, TYPE and COUNT lines don't agree"));
    }
    for (k, field) in fields.iter_mut().enumerate() {
        field.size = sizes.get(k).copied().unwrap_or(0);
        field.kind = types.get(k).copied().unwrap_or(0);
        field.count = counts.get(k).copied().unwrap_or(1);
        let known = matches!((field.kind, field.size), (b'F', 4 | 8) | (b'I' | b'U', 1 | 2 | 4 | 8));
        if !known || field.count == 0 || field.count > 4096 {
            return Err(bad(format!(
                "unsupported PCD field `{}` ({} bytes of type {}, {} each)",
                field.name,
                field.size,
                char::from(field.kind),
                field.count
            )));
        }
    }
    let slot = |name: &str| fields.iter().position(|f| f.name == name);
    let (Some(x), Some(y), Some(z)) = (slot("x"), slot("y"), slot("z")) else { return Err(bad("the PCD file has no x, y and z fields")) };
    if [x, y, z].iter().any(|&k| fields[k].count != 1) {
        return Err(bad("a PCD coordinate field has more than one value"));
    }
    let count = points.or_else(|| width.map(|w| w.saturating_mul(height))).ok_or_else(|| bad("the PCD header has no POINTS or WIDTH line"))?;
    if count > most {
        return Err(too_many(count as u64, most));
    }
    // PCL packs a colour into one four-byte field, `rgb` (a float's bits) or `rgba`: 0x00RRGGBB.
    let rgb = slot("rgb").or_else(|| slot("rgba")).filter(|&k| fields[k].size == 4 && fields[k].count == 1);
    let mut out = Points { points: Vec::with_capacity(count.min(1 << 24)), colors: rgb.map(|_| Vec::with_capacity(count.min(1 << 24))) };
    let mut keep = |p: [f64; 3], c: Option<u32>| {
        // A NaN point is a missing return of an organised cloud.
        if p.iter().all(|v| v.is_finite()) {
            out.points.push(p);
            if let (Some(list), Some(c)) = (out.colors.as_mut(), c) {
                list.push([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            }
        }
    };
    let body = data.get(offset..).unwrap_or_default();
    match format {
        PcdData::Ascii => {
            // The first value of each field in a line.
            let mut starts = Vec::with_capacity(fields.len());
            let mut at = 0;
            for f in &fields {
                starts.push(at);
                at += f.count;
            }
            let width_of_line = at;
            let text = String::from_utf8_lossy(body);
            let mut read = 0;
            for (n, line) in text.split('\n').enumerate() {
                if read == count {
                    break;
                }
                let values: Vec<&str> = line.split_whitespace().collect();
                if values.is_empty() {
                    continue;
                }
                if values.len() < width_of_line {
                    return Err(bad(format!("PCD point {}: {} values, where the fields have {width_of_line}", n + 1, values.len())));
                }
                let word = |k: usize| values.get(starts[k]).copied().unwrap_or("");
                let value = |k: usize| -> Result<f64> {
                    // `nan` is how PCL writes a missing return.
                    word(k).parse::<f64>().map_err(|_| bad(format!("PCD point {}: `{}` is not a number", n + 1, word(k))))
                };
                let p = [value(x)?, value(y)?, value(z)?];
                // A float field's colour is the bits of the float written; an integer field's, the number.
                let colour = rgb.map(|k| {
                    if fields[k].kind == b'F' {
                        word(k).parse::<f32>().map_or(0, f32::to_bits)
                    } else {
                        word(k).parse::<f64>().map_or(0, |v| v as u32)
                    }
                });
                keep(p, colour);
                read += 1;
            }
            if read < count {
                return Err(bad("the PCD data ends early"));
            }
        }
        PcdData::Binary | PcdData::Compressed => {
            let record: usize = fields.iter().map(|f| f.size * f.count).sum();
            let total = count.checked_mul(record).ok_or_else(|| bad("the PCD file is too large"))?;
            let unpacked;
            let (bytes, packed_by_field) = if format == PcdData::Binary {
                if body.len() < total {
                    return Err(bad("the PCD data ends early"));
                }
                (body, false)
            } else {
                let compressed = u32::from_le_bytes(take(body, 0)) as usize;
                let size = u32::from_le_bytes(take(body, 4)) as usize;
                if body.len() < 8 || size != total || body.len() < 8 + compressed {
                    return Err(bad("the PCD compressed data doesn't match its header"));
                }
                let packed = body.get(8..8 + compressed).unwrap_or_default();
                unpacked = lzf_decompress(packed, size).ok_or_else(|| bad("the PCD compressed data is damaged"))?;
                (unpacked.as_slice(), true)
            };
            // Where field `k` of point `i` starts.
            let mut field_offsets = Vec::with_capacity(fields.len());
            let mut at = 0;
            for f in &fields {
                field_offsets.push(at);
                at += f.size * f.count;
            }
            let place = |k: usize, i: usize| {
                if packed_by_field { field_offsets[k] * count + i * fields[k].size * fields[k].count } else { i * record + field_offsets[k] }
            };
            for i in 0..count {
                let p = [fields[x].decode(bytes, place(x, i)), fields[y].decode(bytes, place(y, i)), fields[z].decode(bytes, place(z, i))];
                keep(p, rgb.map(|k| u32::from_le_bytes(take(bytes, place(k, i)))));
            }
        }
    }
    Ok(out)
}

/// LZF (as PCL compresses): runs of literal bytes and copies from earlier in the output. `None`
/// when the data is damaged or does not unpack to exactly `size` bytes.
fn lzf_decompress(input: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut i = 0;
    while i < input.len() {
        let control = usize::from(*input.get(i)?);
        i += 1;
        if control < 32 {
            let run = control + 1;
            if out.len() + run > size {
                return None;
            }
            out.extend_from_slice(input.get(i..i + run)?);
            i += run;
        } else {
            let mut length = control >> 5;
            if length == 7 {
                length += usize::from(*input.get(i)?);
                i += 1;
            }
            length += 2;
            let back = ((control & 0x1f) << 8) + usize::from(*input.get(i)?) + 1;
            i += 1;
            if back > out.len() || out.len() + length > size {
                return None;
            }
            let start = out.len() - back;
            for k in 0..length {
                let byte = *out.get(start + k)?;
                out.push(byte);
            }
        }
    }
    (out.len() == size).then_some(out)
}

// ---- PTX ----

/// The numbers of a line, or `None` when a word is not one.
fn numbers(line: &str) -> Option<Vec<f64>> {
    line.split_whitespace().map(|w| w.parse::<f64>().ok()).collect()
}

fn read_ptx(data: &[u8], most: usize) -> Result<Points> {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).enumerate();
    let mut out = Points::default();
    let mut coloured: Option<bool> = None;
    let mut station = 0;
    let header = |line: Option<(usize, &str)>, what: &str, want: usize, station: usize| -> Result<Vec<f64>> {
        let (n, line) = line.ok_or_else(|| bad(format!("PTX station {station}: the file ends in its header")))?;
        numbers(line)
            .filter(|v| v.len() >= want && v.iter().all(|x| x.is_finite()))
            .ok_or_else(|| bad(format!("PTX line {}: the station's {what} should be {want} numbers", n + 1)))
    };
    while let Some(first) = lines.next() {
        station += 1;
        let columns = header(Some(first), "column count", 1, station)?[0];
        let rows = header(lines.next(), "row count", 1, station)?[0];
        if !(columns >= 0.0 && rows >= 0.0 && columns.fract() == 0.0 && rows.fract() == 0.0) {
            return Err(bad(format!("PTX station {station}: its columns and rows are not whole numbers")));
        }
        let grid = (columns * rows) as u64;
        // The scanner's place and its three axes: read past (the matrix registers the points).
        for what in ["scanner's place", "first axis", "second axis", "third axis"] {
            header(lines.next(), what, 3, station)?;
        }
        let mut matrix = [[0.0f64; 4]; 4];
        for row in &mut matrix {
            let v = header(lines.next(), "matrix row", 4, station)?;
            row.copy_from_slice(&v[..4]);
        }
        if out.points.len() as u64 + grid > most as u64 {
            return Err(too_many(out.points.len() as u64 + grid, most));
        }
        for _ in 0..grid {
            let (n, line) = lines.next().ok_or_else(|| bad(format!("PTX station {station}: the file ends before its {grid} points")))?;
            let v = numbers(line).filter(|v| v.len() >= 3).ok_or_else(|| bad(format!("PTX line {}: a point should be numbers", n + 1)))?;
            let colour = (v.len() >= 7).then(|| [v[4], v[5], v[6]].map(|c| c.clamp(0.0, 255.0) as u8));
            if *coloured.get_or_insert(colour.is_some()) != colour.is_some() {
                coloured = Some(false);
            }
            let (x, y, z) = (v[0], v[1], v[2]);
            // A missing return.
            if x == 0.0 && y == 0.0 && z == 0.0 {
                continue;
            }
            let at = |k: usize| x * matrix[0][k] + y * matrix[1][k] + z * matrix[2][k] + matrix[3][k];
            out.points.push([at(0), at(1), at(2)]);
            if let Some(c) = colour {
                out.colors.get_or_insert_with(Vec::new).push(c);
            }
        }
    }
    // Colours only when every point has one.
    if coloured != Some(true) || out.colors.as_ref().is_some_and(|c| c.len() != out.points.len()) {
        out.colors = None;
    }
    Ok(out)
}

// ---- E57 and LAS ----

/// The range of colour values a scan's colour field takes: the scan's colour limits, else its
/// data type's.
fn colour_range(limits: Option<(&Option<e57::RecordValue>, &Option<e57::RecordValue>)>, dt: &e57::RecordDataType) -> (f64, f64) {
    use e57::RecordDataType as T;
    let typed = match dt {
        T::Integer { min, max } => (*min as f64, *max as f64),
        T::ScaledInteger { min, max, scale, offset } => (*min as f64 * scale + offset, *max as f64 * scale + offset),
        T::Single { min, max } => (min.map_or(0.0, f64::from), max.map_or(1.0, f64::from)),
        T::Double { min, max } => (min.unwrap_or(0.0), max.unwrap_or(1.0)),
    };
    let given = |v: &Option<e57::RecordValue>| v.as_ref().and_then(|v| v.to_f64(dt).ok());
    match limits {
        Some((lo, hi)) => (given(lo).unwrap_or(typed.0), given(hi).unwrap_or(typed.1)),
        None => typed,
    }
}

/// The rotation of a unit quaternion (w, x, y, z), row by row.
fn rotation(q: &e57::Quaternion) -> [[f64; 3]; 3] {
    let (w, x, y, z) = (q.w, q.x, q.y, q.z);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Read through the crate's raw reader, which reads on until each field has a value (its simple
/// reader gave up on a scan whose invalid-state field packs a few bits a point).
fn read_e57(data: &[u8], most: usize) -> Result<Points> {
    use e57::RecordName as N;
    let unreadable = |e: e57::Error| bad(format!("the E57 file couldn't be read: {e}"));
    let mut reader = e57::E57Reader::new(Cursor::new(data)).map_err(unreadable)?;
    let clouds = reader.pointclouds();
    let declared: u64 = clouds.iter().map(|c| c.records).sum();
    if declared > most as u64 {
        return Err(too_many(declared, most));
    }
    let size = usize::try_from(declared).unwrap_or(0);
    let field = |cloud: &e57::PointCloud, name: N| cloud.prototype.iter().position(|r| r.name == name);
    let has_colour = |cloud: &e57::PointCloud| [N::ColorRed, N::ColorGreen, N::ColorBlue].into_iter().all(|n| field(cloud, n).is_some());
    // Colours when every scan has them.
    let coloured = !clouds.is_empty() && clouds.iter().all(has_colour);
    let mut out = Points { points: Vec::with_capacity(size), colors: coloured.then(|| Vec::with_capacity(size)) };
    for cloud in &clouds {
        let at = |name: N| field(cloud, name);
        let cartesian = [at(N::CartesianX), at(N::CartesianY), at(N::CartesianZ)];
        let spherical = [at(N::SphericalRange), at(N::SphericalAzimuth), at(N::SphericalElevation)];
        let (c_invalid, s_invalid) = (at(N::CartesianInvalidState), at(N::SphericalInvalidState));
        let colour = [at(N::ColorRed), at(N::ColorGreen), at(N::ColorBlue)];
        let limits = cloud.color_limits.as_ref();
        let ranges: Vec<(f64, f64)> = colour
            .iter()
            .zip(0..)
            .map(|(k, c)| {
                let dt = k.and_then(|k| cloud.prototype.get(k)).map(|r| &r.data_type);
                let given = limits.map(|l| match c {
                    0 => (&l.red_min, &l.red_max),
                    1 => (&l.green_min, &l.green_max),
                    _ => (&l.blue_min, &l.blue_max),
                });
                dt.map_or((0.0, 1.0), |dt| colour_range(given, dt))
            })
            .collect();
        let pose = cloud.transform.as_ref().map(|t| (rotation(&t.rotation), [t.translation.x, t.translation.y, t.translation.z]));
        let raw = reader.pointcloud_raw(cloud).map_err(unreadable)?;
        for values in raw {
            let values = values.map_err(unreadable)?;
            let number = |k: Option<usize>| -> Option<f64> {
                let k = k?;
                values.get(k)?.to_f64(&cloud.prototype.get(k)?.data_type).ok()
            };
            let state = |k: Option<usize>| k.and_then(|k| values.get(k)?.to_i64(&cloud.prototype.get(k)?.data_type).ok()).unwrap_or(0);
            let p = match (cartesian, spherical) {
                ([Some(_), Some(_), Some(_)], _) if state(c_invalid) == 0 => [number(cartesian[0]), number(cartesian[1]), number(cartesian[2])],
                (_, [Some(_), Some(_), Some(_)]) if state(s_invalid) == 0 => {
                    match (number(spherical[0]), number(spherical[1]), number(spherical[2])) {
                        (Some(r), Some(a), Some(e)) => [Some(r * e.cos() * a.cos()), Some(r * e.cos() * a.sin()), Some(r * e.sin())],
                        _ => [None; 3],
                    }
                }
                _ => [None; 3],
            };
            let [Some(x), Some(y), Some(z)] = p else { continue };
            let p = match &pose {
                Some((r, t)) => [0, 1, 2].map(|i| r[i][0] * x + r[i][1] * y + r[i][2] * z + t[i]),
                None => [x, y, z],
            };
            if !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            out.points.push(p);
            if let Some(list) = out.colors.as_mut() {
                let byte = |i: usize| {
                    let (lo, hi) = ranges.get(i).copied().unwrap_or((0.0, 1.0));
                    let v = number(colour[i]).unwrap_or(lo);
                    let share = if hi > lo { (v - lo) / (hi - lo) } else { 0.0 };
                    (share * 255.0).round().clamp(0.0, 255.0) as u8
                };
                list.push([byte(0), byte(1), byte(2)]);
            }
        }
    }
    Ok(out)
}

fn read_las(data: &[u8], most: usize) -> Result<Points> {
    let unreadable = |e: las::Error| bad(format!("the LAS file couldn't be read: {e}"));
    // The reader holds what it reads from for good: a copy of the bytes.
    let mut reader = las::Reader::new(Cursor::new(data.to_vec())).map_err(unreadable)?;
    let declared = reader.header().number_of_points();
    if declared > most as u64 {
        return Err(too_many(declared, most));
    }
    let size = usize::try_from(declared).unwrap_or(0);
    let has_color = reader.header().point_format().has_color;
    let mut points = Vec::with_capacity(size);
    let mut wide: Option<Vec<[u16; 3]>> = has_color.then(|| Vec::with_capacity(size));
    loop {
        let chunk = reader.read_points(1 << 16).map_err(unreadable)?;
        if chunk.is_empty() {
            break;
        }
        for point in chunk.points() {
            let point = point.map_err(unreadable)?;
            points.push([point.x, point.y, point.z]);
            if let Some(list) = wide.as_mut() {
                list.push(point.color.map_or([0, 0, 0], |c| [c.red, c.green, c.blue]));
            }
            if points.len() > most {
                return Err(too_many(points.len() as u64, most));
            }
        }
    }
    // LAS colours are 16 bits a channel; most writers fill all 16, some only the low 8.
    let colors = wide.map(|list| {
        let full = list.iter().any(|c| c.iter().any(|v| *v > 255));
        list.into_iter().map(|c| c.map(|v| if full { (v >> 8) as u8 } else { v as u8 })).collect()
    });
    Ok(Points { points, colors })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII_PCD: &str = "# .PCD v0.7\nVERSION 0.7\nFIELDS x y z rgb\nSIZE 4 4 4 4\nTYPE F F F U\nCOUNT 1 1 1 1\nWIDTH 3\nHEIGHT 1\nVIEWPOINT 0 0 0 1 0 0 0\nPOINTS 3\nDATA ascii\n1 2 3 16711680\nnan nan nan 0\n4 5 6 65280\n";

    #[test]
    fn a_text_pcd_leaves_missing_returns_out_and_keeps_colours() {
        let read = read(ASCII_PCD.as_bytes(), "pcd", 10).unwrap();
        assert_eq!(read.points, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(read.colors, Some(vec![[255, 0, 0], [0, 255, 0]]));
        assert!(super::read(ASCII_PCD.as_bytes(), "pcd", 2).is_err(), "three points, two allowed");
    }

    /// A packed PCD, record by record, and the same compressed field by field with LZF (literal
    /// runs only, as a compressor may write): the same points.
    #[test]
    fn a_packed_pcd_and_a_compressed_one_read_alike() {
        let header =
            |data: &str| format!("VERSION 0.7\nFIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nCOUNT 1 1 1\nWIDTH 2\nHEIGHT 1\nPOINTS 2\nDATA {data}\n");
        let points = [[1.0f32, 2.0, 3.0], [-4.0, 5.5, 6.25]];
        let mut packed = header("binary").into_bytes();
        for p in points {
            for v in p {
                packed.extend(v.to_le_bytes());
            }
        }
        let want: Vec<[f64; 3]> = points.iter().map(|p| p.map(f64::from)).collect();
        assert_eq!(read(&packed, "pcd", 10).unwrap().points, want);
        // Field by field: all x, then all y, then all z.
        let mut raw = Vec::new();
        for k in 0..3 {
            for p in points {
                raw.extend(p[k].to_le_bytes());
            }
        }
        let mut lzf = Vec::new();
        for chunk in raw.chunks(32) {
            lzf.push((chunk.len() - 1) as u8);
            lzf.extend_from_slice(chunk);
        }
        let mut compressed = header("binary_compressed").into_bytes();
        compressed.extend((lzf.len() as u32).to_le_bytes());
        compressed.extend((raw.len() as u32).to_le_bytes());
        compressed.extend(&lzf);
        assert_eq!(read(&compressed, "pcd", 10).unwrap().points, want);
        // Cut short: refused, not a panic.
        assert!(read(&packed[..packed.len() - 3], "pcd", 10).is_err());
        assert!(read(&compressed[..compressed.len() - 3], "pcd", 10).is_err());
    }

    /// A PTX station of 2 × 2 points: its matrix moves them (by 10 along x here), the missing
    /// return is left out, and the colours are kept.
    #[test]
    fn a_ptx_station_is_registered_by_its_matrix() {
        let ptx = "2\n2\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n10 0 0 1\n1 2 3 0.5 255 0 0\n0 0 0 0 0 0 0\n4 5 6 0.5 0 255 0\n7 8 9 0.5 0 0 255\n";
        let read = read(ptx.as_bytes(), "ptx", 10).unwrap();
        assert_eq!(read.points, vec![[11.0, 2.0, 3.0], [14.0, 5.0, 6.0], [17.0, 8.0, 9.0]]);
        assert_eq!(read.colors, Some(vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]]));
        assert!(super::read(&ptx.as_bytes()[..ptx.len() - 20], "ptx", 10).is_err(), "cut short");
    }

    /// An E57 file written with the crate's own writer: one scan of three points, the third
    /// marked invalid, read back as its two valid ones; one of spherical points with colours, its
    /// pose moving them (a quarter turn about z and 10 along x).
    #[test]
    fn an_e57_scan_reads_its_valid_points() {
        use e57::{Record, RecordValue};
        let mut file = Cursor::new(Vec::new());
        {
            let mut writer = e57::E57Writer::new(&mut file, "file-guid").unwrap();
            let prototype = vec![Record::CARTESIAN_X_F64, Record::CARTESIAN_Y_F64, Record::CARTESIAN_Z_F64, Record::CARTESIAN_INVALID_STATE];
            let mut cloud = writer.add_pointcloud("cloud-guid", prototype).unwrap();
            for (p, invalid) in [([1.0, 2.0, 3.0], 0), ([4.0, 5.0, 6.0], 0), ([7.0, 8.0, 9.0], 2)] {
                let mut values: Vec<RecordValue> = p.iter().map(|v| RecordValue::Double(*v)).collect();
                values.push(RecordValue::Integer(invalid));
                cloud.add_point(values).unwrap();
            }
            cloud.finalize().unwrap();
            writer.finalize().unwrap();
        }
        let cartesian = read(file.get_ref(), "e57", 10).unwrap();
        assert_eq!(cartesian.points, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(cartesian.colors, None);

        let mut file = Cursor::new(Vec::new());
        {
            let mut writer = e57::E57Writer::new(&mut file, "file-guid").unwrap();
            let prototype = vec![
                Record::SPHERICAL_RANGE_F64,
                Record::SPHERICAL_AZIMUTH_F64,
                Record::SPHERICAL_ELEVATION_F64,
                Record::COLOR_RED_U8,
                Record::COLOR_GREEN_U8,
                Record::COLOR_BLUE_U8,
            ];
            let mut cloud = writer.add_pointcloud("cloud-guid", prototype).unwrap();
            let half = std::f64::consts::FRAC_PI_4;
            cloud.set_transform(Some(e57::Transform {
                rotation: e57::Quaternion { w: half.cos(), x: 0.0, y: 0.0, z: half.sin() },
                translation: e57::Translation { x: 10.0, y: 0.0, z: 0.0 },
            }));
            for (range, azimuth, elevation, colour) in [(2.0, 0.0, 0.0, [255, 128, 0]), (3.0, 0.0, std::f64::consts::FRAC_PI_2, [0, 0, 255])] {
                let mut values = vec![RecordValue::Double(range), RecordValue::Double(azimuth), RecordValue::Double(elevation)];
                values.extend(colour.map(RecordValue::Integer));
                cloud.add_point(values).unwrap();
            }
            cloud.finalize().unwrap();
            writer.finalize().unwrap();
        }
        let spherical = read(file.get_ref(), "e57", 10).unwrap();
        // (2, 0, 0) turned a quarter about z is (0, 2, 0), moved to (10, 2, 0); straight up 3 is (10, 0, 3).
        let want = [[10.0, 2.0, 0.0], [10.0, 0.0, 3.0]];
        for (got, want) in spherical.points.iter().zip(want) {
            assert!(got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9), "{got:?} against {want:?}");
        }
        assert_eq!(spherical.points.len(), 2);
        assert_eq!(spherical.colors, Some(vec![[255, 128, 0], [0, 0, 255]]));
    }

    /// A LAS file of format 2 (with colour) written with the crate's own writer reads back its
    /// points at the header's scale and its full 16-bit colours as bytes.
    #[test]
    fn a_las_file_reads_its_points_and_colours() {
        let mut builder = las::Builder::default();
        builder.point_format = las::point::Format::new(2).unwrap();
        let header = builder.into_header().unwrap();
        let mut writer = las::Writer::new(Cursor::new(Vec::new()), header).unwrap();
        for (p, c) in [([1.0, 2.0, 3.0], [65535, 0, 0]), ([4.5, -5.0, 6.25], [0, 65535, 32768])] {
            let point = las::Point { x: p[0], y: p[1], z: p[2], color: Some(las::Color::new(c[0], c[1], c[2])), ..Default::default() };
            writer.write_point(point).unwrap();
        }
        let bytes = writer.into_inner().unwrap().into_inner();
        let read = read(&bytes, "las", 10).unwrap();
        assert_eq!(read.points, vec![[1.0, 2.0, 3.0], [4.5, -5.0, 6.25]]);
        assert_eq!(read.colors, Some(vec![[255, 0, 0], [0, 255, 128]]));
        assert!(super::read(&bytes, "las", 1).is_err(), "two points, one allowed");
    }

    #[test]
    fn hostile_files_are_refused_not_crashed_on() {
        for ext in EXTENSIONS {
            for bytes in [&b""[..], b"\0\0\0\0garbage", b"FIELDS x y z\nDATA binary\n", b"5\n5\n"] {
                assert!(read(bytes, ext, 1000).is_err(), "{ext}: {bytes:?}");
            }
        }
        assert!(lzf_decompress(&[0xff, 0xff], 10).is_none());
        assert!(lzf_decompress(&[31, 1], 32).is_none());
    }
}
