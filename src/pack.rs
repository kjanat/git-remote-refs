use crate::{Error, Kind, ObjectFormat, ObjectId, object::object_id, oid::Hasher};
use alloc::{boxed::Box, collections::BTreeMap, vec, vec::Vec};
use miniz_oxide::inflate::{
    TINFLStatus,
    core::{DecompressorOxide, decompress, inflate_flags},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_objects: u32,
    pub max_object_size: usize,
    pub max_total_size: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_objects: 100_000,
            max_object_size: 64 << 20,
            max_total_size: 256 << 20,
        }
    }
}

enum Base {
    Offset(usize),
    Id(ObjectId),
}

struct Pending {
    index: usize,
    base: Base,
    delta: Vec<u8>,
}

struct Budget<'a> {
    limits: &'a Limits,
    used: usize,
}

impl Budget<'_> {
    fn reserve(&mut self, size: u64) -> Result<usize, Error> {
        let size = usize::try_from(size).map_err(|_| Error::LimitExceeded("object size"))?;
        if size > self.limits.max_object_size {
            return Err(Error::LimitExceeded("object size"));
        }
        self.used = self
            .used
            .checked_add(size)
            .filter(|&used| used <= self.limits.max_total_size)
            .ok_or(Error::LimitExceeded("total object size"))?;
        Ok(size)
    }
}

fn byte(input: &[u8], pos: &mut usize) -> Result<u8, Error> {
    let value = *input.get(*pos).ok_or(Error::Protocol("truncated pack"))?;
    *pos += 1;
    Ok(value)
}

fn varint(input: &[u8], pos: &mut usize) -> Result<u64, Error> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let next = byte(input, pos)?;
        if shift > 56 {
            return Err(Error::Protocol("size overflow"));
        }
        value |= u64::from(next & 0x7f) << shift;
        shift += 7;
        if next & 0x80 == 0 {
            return Ok(value);
        }
    }
}

fn inflate(input: &[u8], pos: &mut usize, size: usize) -> Result<Vec<u8>, Error> {
    let mut output = vec![0; size];
    let mut state = Box::<DecompressorOxide>::default();
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
        | inflate_flags::TINFL_FLAG_COMPUTE_ADLER32
        | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, consumed, written) = decompress(&mut state, &input[*pos..], &mut output, 0, flags);
    if status != TINFLStatus::Done || written != size {
        return Err(Error::Protocol("invalid compressed object"));
    }
    *pos += consumed;
    Ok(output)
}

fn apply_delta(base: &[u8], delta: &[u8], budget: &mut Budget) -> Result<Vec<u8>, Error> {
    let error = || Error::Protocol("invalid delta");
    let mut pos = 0;
    if varint(delta, &mut pos)? != base.len() as u64 {
        return Err(Error::Protocol("delta base size mismatch"));
    }
    let size = budget.reserve(varint(delta, &mut pos)?)?;
    let mut output = Vec::with_capacity(size);
    while pos < delta.len() {
        let op = byte(delta, &mut pos)?;
        let chunk = if op & 0x80 != 0 {
            let mut offset = 0usize;
            let mut len = 0usize;
            for bit in 0..4 {
                if op & (1 << bit) != 0 {
                    offset |= usize::from(byte(delta, &mut pos)?) << (8 * bit);
                }
            }
            for bit in 0..3 {
                if op & (0x10 << bit) != 0 {
                    len |= usize::from(byte(delta, &mut pos)?) << (8 * bit);
                }
            }
            if len == 0 {
                len = 0x10000;
            }
            base.get(offset..offset.checked_add(len).ok_or_else(error)?)
                .ok_or_else(error)?
        } else if op != 0 {
            let end = pos + usize::from(op);
            let insert = delta.get(pos..end).ok_or_else(error)?;
            pos = end;
            insert
        } else {
            return Err(error());
        };
        if output.len() + chunk.len() > size {
            return Err(error());
        }
        output.extend_from_slice(chunk);
    }
    if output.len() != size {
        return Err(error());
    }
    Ok(output)
}

pub(crate) fn parse(
    format: ObjectFormat,
    pack: &[u8],
    limits: &Limits,
) -> Result<BTreeMap<ObjectId, (Kind, Vec<u8>)>, Error> {
    let trailer_len = format.raw_len();
    if pack.len() < 12 + trailer_len || &pack[..4] != b"PACK" {
        return Err(Error::Protocol("invalid pack header"));
    }
    let version = u32::from_be_bytes([pack[4], pack[5], pack[6], pack[7]]);
    if !matches!(version, 2 | 3) {
        return Err(Error::Protocol("unsupported pack version"));
    }
    let count = u32::from_be_bytes([pack[8], pack[9], pack[10], pack[11]]);
    if count > limits.max_objects {
        return Err(Error::LimitExceeded("object count"));
    }
    let (content, trailer) = pack.split_at(pack.len() - trailer_len);
    let mut hasher = Hasher::new(format);
    hasher.update(content);
    if hasher.finish().as_bytes() != trailer {
        return Err(Error::ChecksumMismatch);
    }

    let mut budget = Budget { limits, used: 0 };
    let mut resolved: Vec<Option<(Kind, Vec<u8>)>> = Vec::new();
    let mut pending = Vec::new();
    let mut offsets = BTreeMap::new();
    let mut pos = 12;
    for index in 0..count as usize {
        let start = pos;
        let first = byte(content, &mut pos)?;
        let mut size = u64::from(first & 15);
        let mut shift = 4;
        let mut next = first;
        while next & 0x80 != 0 {
            next = byte(content, &mut pos)?;
            if shift > 57 {
                return Err(Error::Protocol("size overflow"));
            }
            size |= u64::from(next & 0x7f) << shift;
            shift += 7;
        }
        offsets.insert(start, index);
        let code = (first >> 4) & 7;
        match code {
            6 => {
                let mut distance = u64::from(byte(content, &mut pos)? & 0x7f);
                let mut current = content[pos - 1];
                while current & 0x80 != 0 {
                    current = byte(content, &mut pos)?;
                    distance = (distance + 1)
                        .checked_mul(128)
                        .ok_or(Error::Protocol("invalid delta base offset"))?
                        | u64::from(current & 0x7f);
                }
                let base = usize::try_from(distance)
                    .ok()
                    .and_then(|distance| start.checked_sub(distance))
                    .ok_or(Error::Protocol("invalid delta base offset"))?;
                let size = budget.reserve(size)?;
                let delta = inflate(content, &mut pos, size)?;
                pending.push(Pending {
                    index,
                    base: Base::Offset(base),
                    delta,
                });
                resolved.push(None);
            }
            7 => {
                let raw = content
                    .get(pos..pos + trailer_len)
                    .ok_or(Error::Protocol("truncated pack"))?;
                let base =
                    ObjectId::from_raw(format, raw).ok_or(Error::Protocol("truncated pack"))?;
                pos += trailer_len;
                let size = budget.reserve(size)?;
                let delta = inflate(content, &mut pos, size)?;
                pending.push(Pending {
                    index,
                    base: Base::Id(base),
                    delta,
                });
                resolved.push(None);
            }
            _ => {
                let kind =
                    Kind::from_pack_type(code).ok_or(Error::Protocol("invalid object type"))?;
                let size = budget.reserve(size)?;
                resolved.push(Some((kind, inflate(content, &mut pos, size)?)));
            }
        }
    }
    if pos != content.len() {
        return Err(Error::Protocol("unexpected data after pack objects"));
    }

    let mut ids: Vec<Option<ObjectId>> = resolved
        .iter()
        .map(|entry| {
            entry
                .as_ref()
                .map(|(kind, data)| object_id(format, *kind, data))
        })
        .collect();
    let mut by_id: BTreeMap<ObjectId, usize> = ids
        .iter()
        .enumerate()
        .filter_map(|(index, id)| id.map(|id| (id, index)))
        .collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut waiting = Vec::new();
        for item in pending {
            let base_index = match &item.base {
                Base::Offset(offset) => offsets.get(offset).copied(),
                Base::Id(id) => by_id.get(id).copied(),
            };
            let Some((kind, base)) = base_index.and_then(|index| resolved[index].as_ref()) else {
                waiting.push(item);
                continue;
            };
            let kind = *kind;
            let data = apply_delta(base, &item.delta, &mut budget)?;
            let id = object_id(format, kind, &data);
            by_id.insert(id, item.index);
            ids[item.index] = Some(id);
            resolved[item.index] = Some((kind, data));
        }
        if waiting.len() == before {
            return Err(Error::Protocol("delta base missing from pack"));
        }
        pending = waiting;
    }

    Ok(ids
        .into_iter()
        .zip(resolved)
        .filter_map(|(id, entry)| Some((id?, entry?)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    fn header(code: u8, size: usize) -> Vec<u8> {
        let mut out = vec![(code << 4) | (size & 15) as u8];
        let mut rest = size >> 4;
        while rest > 0 {
            *out.last_mut().unwrap() |= 0x80;
            out.push((rest & 0x7f) as u8);
            rest >>= 7;
        }
        out
    }

    fn pack(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"PACK\0\0\0\x02".to_vec();
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for entry in entries {
            out.extend_from_slice(entry);
        }
        let mut hasher = Hasher::new(ObjectFormat::Sha1);
        hasher.update(&out);
        out.extend_from_slice(hasher.finish().as_bytes());
        out
    }

    fn full(code: u8, data: &[u8]) -> Vec<u8> {
        let mut out = header(code, data.len());
        out.extend(compress_to_vec_zlib(data, 6));
        out
    }

    const BASE: &[u8] = b"hello world, this is the base blob\n";

    fn delta() -> Vec<u8> {
        let mut delta = vec![BASE.len() as u8, 17];
        delta.extend_from_slice(&[0x91, 0, 11]);
        delta.extend_from_slice(&[6]);
        delta.extend_from_slice(b"!!!!!\n");
        delta
    }

    #[test]
    fn resolves_offset_and_ref_deltas() {
        let base = full(3, BASE);
        let base_id = object_id(ObjectFormat::Sha1, Kind::Blob, BASE);
        let mut ofs = header(6, delta().len());
        ofs.push(base.len() as u8);
        ofs.extend(compress_to_vec_zlib(&delta(), 6));
        let mut by_ref = header(7, delta().len());
        by_ref.extend_from_slice(base_id.as_bytes());
        by_ref.extend(compress_to_vec_zlib(&delta(), 6));
        let objects = parse(
            ObjectFormat::Sha1,
            &pack(&[base, ofs, by_ref]),
            &Limits::default(),
        )
        .unwrap();
        let expected = b"hello world!!!!!\n";
        let derived = object_id(ObjectFormat::Sha1, Kind::Blob, expected);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[&derived], (Kind::Blob, expected.to_vec()));
        assert_eq!(objects[&base_id], (Kind::Blob, BASE.to_vec()));
    }

    #[test]
    fn resolves_ref_delta_before_its_base() {
        let base = full(3, BASE);
        let base_id = object_id(ObjectFormat::Sha1, Kind::Blob, BASE);
        let mut by_ref = header(7, delta().len());
        by_ref.extend_from_slice(base_id.as_bytes());
        by_ref.extend(compress_to_vec_zlib(&delta(), 6));
        let objects = parse(
            ObjectFormat::Sha1,
            &pack(&[by_ref, base]),
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(objects.len(), 2);
    }

    fn offset(mut distance: usize) -> Vec<u8> {
        let mut out = vec![(distance & 0x7f) as u8];
        distance >>= 7;
        while distance > 0 {
            distance -= 1;
            out.insert(0, 0x80 | (distance & 0x7f) as u8);
            distance >>= 7;
        }
        out
    }

    #[test]
    fn resolves_distant_offset_deltas_in_version_3_packs() {
        let data: Vec<u8> = (0..4000u32)
            .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let base = full(3, &data);
        assert!(base.len() > 300);
        let mut delta = vec![0xa0, 0x1f, 4];
        delta.extend_from_slice(&[0x91, 0, 4]);
        let mut entry = header(6, delta.len());
        entry.extend(offset(base.len()));
        entry.extend(compress_to_vec_zlib(&delta, 6));
        let mut packed = pack(&[base, entry]);
        packed[7] = 3;
        let trailer = packed.len() - 20;
        let mut hasher = Hasher::new(ObjectFormat::Sha1);
        hasher.update(&packed[..trailer]);
        packed.truncate(trailer);
        packed.extend_from_slice(hasher.finish().as_bytes());
        let objects = parse(ObjectFormat::Sha1, &packed, &Limits::default()).unwrap();
        let derived = object_id(ObjectFormat::Sha1, Kind::Blob, &data[..4]);
        assert_eq!(objects[&derived].1, data[..4]);
    }

    #[test]
    fn enforces_total_size_across_objects() {
        let tight = Limits {
            max_total_size: BASE.len() * 2 - 1,
            ..Limits::default()
        };
        assert_eq!(
            parse(
                ObjectFormat::Sha1,
                &pack(&[full(3, BASE), full(3, BASE)]),
                &tight
            ),
            Err(Error::LimitExceeded("total object size"))
        );
    }

    #[test]
    fn reads_empty_objects() {
        let objects = parse(
            ObjectFormat::Sha1,
            &pack(&[full(3, b"")]),
            &Limits::default(),
        )
        .unwrap();
        let id = object_id(ObjectFormat::Sha1, Kind::Blob, b"");
        assert_eq!(objects[&id], (Kind::Blob, Vec::new()));
    }

    #[test]
    fn rejects_corruption_and_enforces_limits() {
        let mut corrupt = pack(&[full(3, BASE)]);
        corrupt[14] ^= 1;
        assert_eq!(
            parse(ObjectFormat::Sha1, &corrupt, &Limits::default()),
            Err(Error::ChecksumMismatch)
        );
        let small = Limits {
            max_object_size: 4,
            ..Limits::default()
        };
        assert_eq!(
            parse(ObjectFormat::Sha1, &pack(&[full(3, BASE)]), &small),
            Err(Error::LimitExceeded("object size"))
        );
        let few = Limits {
            max_objects: 0,
            ..Limits::default()
        };
        assert_eq!(
            parse(ObjectFormat::Sha1, &pack(&[full(3, BASE)]), &few),
            Err(Error::LimitExceeded("object count"))
        );
        let mut missing = header(7, delta().len());
        missing.extend_from_slice(&[9; 20]);
        missing.extend(compress_to_vec_zlib(&delta(), 6));
        assert_eq!(
            parse(ObjectFormat::Sha1, &pack(&[missing]), &Limits::default()),
            Err(Error::Protocol("delta base missing from pack"))
        );
        let mut wrong_size = header(3, BASE.len() + 1);
        wrong_size.extend(compress_to_vec_zlib(BASE, 6));
        assert!(parse(ObjectFormat::Sha1, &pack(&[wrong_size]), &Limits::default()).is_err());
        assert!(
            parse(
                ObjectFormat::Sha1,
                b"PACK\0\0\0\x04\0\0\0\0aaaaaaaaaaaaaaaaaaaa",
                &Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_out_of_range_delta_copies() {
        let mut budget = Budget {
            limits: &Limits::default(),
            used: 0,
        };
        assert!(apply_delta(BASE, &[BASE.len() as u8, 4, 0x91, 200, 4], &mut budget).is_err());
        assert!(apply_delta(BASE, &[BASE.len() as u8, 4, 0], &mut budget).is_err());
        assert!(apply_delta(BASE, &[1, 4, 4, b'a', b'b', b'c', b'd'], &mut budget).is_err());
        assert_eq!(
            apply_delta(BASE, &[BASE.len() as u8, 2, 2, b'o', b'k'], &mut budget),
            Ok(b"ok".to_vec())
        );
    }
}
