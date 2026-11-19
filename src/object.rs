use crate::{Error, ObjectFormat, ObjectId, oid::Hasher};
use alloc::{format, vec::Vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Blob => "blob",
            Self::Tag => "tag",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"commit" => Some(Self::Commit),
            b"tree" => Some(Self::Tree),
            b"blob" => Some(Self::Blob),
            b"tag" => Some(Self::Tag),
            _ => None,
        }
    }

    pub(crate) const fn from_pack_type(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Commit),
            2 => Some(Self::Tree),
            3 => Some(Self::Blob),
            4 => Some(Self::Tag),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: Vec<u8>,
    pub email: Vec<u8>,
    pub time: i64,
    pub offset_minutes: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: ObjectId,
    pub parents: Vec<ObjectId>,
    pub author: Signature,
    pub committer: Signature,
    pub encoding: Option<Vec<u8>>,
    pub message: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub target: ObjectId,
    pub target_kind: Kind,
    pub name: Vec<u8>,
    pub tagger: Option<Signature>,
    pub message: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: Vec<u8>,
    pub oid: ObjectId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Object {
    Commit(Commit),
    Tree(Vec<TreeEntry>),
    Blob(Vec<u8>),
    Tag(Tag),
}

impl Object {
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Commit(_) => Kind::Commit,
            Self::Tree(_) => Kind::Tree,
            Self::Blob(_) => Kind::Blob,
            Self::Tag(_) => Kind::Tag,
        }
    }

    pub fn parse(format: ObjectFormat, kind: Kind, data: &[u8]) -> Result<Self, Error> {
        Ok(match kind {
            Kind::Commit => Self::Commit(parse_commit(format, data)?),
            Kind::Tree => Self::Tree(parse_tree(format, data)?),
            Kind::Blob => Self::Blob(data.to_vec()),
            Kind::Tag => Self::Tag(parse_tag(format, data)?),
        })
    }
}

pub fn object_id(format: ObjectFormat, kind: Kind, data: &[u8]) -> ObjectId {
    let mut hasher = Hasher::new(format);
    hasher.update(format!("{} {}\0", kind.name(), data.len()).as_bytes());
    hasher.update(data);
    hasher.finish()
}

fn invalid(kind: &'static str) -> Error {
    Error::Protocol(kind)
}

fn headers(data: &[u8]) -> (impl Iterator<Item = (&[u8], &[u8])>, &[u8]) {
    let (head, message) = match data.windows(2).position(|pair| pair == b"\n\n") {
        Some(end) => (&data[..end], &data[end + 2..]),
        None => (data, &[][..]),
    };
    let lines = head
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty() && !line.starts_with(b" "))
        .map(|line| match line.iter().position(|&byte| byte == b' ') {
            Some(space) => (&line[..space], &line[space + 1..]),
            None => (line, &[][..]),
        });
    (lines, message)
}

fn hex(format: ObjectFormat, value: &[u8]) -> Result<ObjectId, Error> {
    ObjectId::parse_hex(format, value).ok_or(invalid("invalid object ID in object"))
}

fn parse_signature(value: &[u8]) -> Result<Signature, Error> {
    let error = || invalid("invalid signature");
    let open = value
        .iter()
        .position(|&byte| byte == b'<')
        .ok_or_else(error)?;
    let close = open
        + value[open..]
            .iter()
            .position(|&byte| byte == b'>')
            .ok_or_else(error)?;
    let name = value[..open].strip_suffix(b" ").unwrap_or(&value[..open]);
    let mut rest = value[close + 1..].trim_ascii().split(|&byte| byte == b' ');
    let (Some(time), Some(zone), None) = (rest.next(), rest.next(), rest.next()) else {
        return Err(error());
    };
    let time = core::str::from_utf8(time)
        .ok()
        .and_then(|time| time.parse().ok())
        .ok_or_else(error)?;
    let (sign, digits) = match zone.split_first() {
        Some((b'+', digits)) => (1, digits),
        Some((b'-', digits)) => (-1, digits),
        _ => return Err(error()),
    };
    if digits.len() != 4 || !digits.iter().all(u8::is_ascii_digit) {
        return Err(error());
    }
    let number = |pair: &[u8]| i32::from(pair[0] - b'0') * 10 + i32::from(pair[1] - b'0');
    Ok(Signature {
        name: name.to_vec(),
        email: value[open + 1..close].to_vec(),
        time,
        offset_minutes: sign * (number(&digits[..2]) * 60 + number(&digits[2..])),
    })
}

fn parse_commit(format: ObjectFormat, data: &[u8]) -> Result<Commit, Error> {
    let (lines, message) = headers(data);
    let (mut tree, mut parents, mut author, mut committer, mut encoding) =
        (None, Vec::new(), None, None, None);
    for (key, value) in lines {
        match key {
            b"tree" if tree.is_none() => tree = Some(hex(format, value)?),
            b"parent" => parents.push(hex(format, value)?),
            b"author" if author.is_none() => author = Some(parse_signature(value)?),
            b"committer" if committer.is_none() => committer = Some(parse_signature(value)?),
            b"encoding" if encoding.is_none() => encoding = Some(value.to_vec()),
            _ => {}
        }
    }
    Ok(Commit {
        tree: tree.ok_or(invalid("commit without tree"))?,
        parents,
        author: author.ok_or(invalid("commit without author"))?,
        committer: committer.ok_or(invalid("commit without committer"))?,
        encoding,
        message: message.to_vec(),
    })
}

fn parse_tag(format: ObjectFormat, data: &[u8]) -> Result<Tag, Error> {
    let (lines, message) = headers(data);
    let (mut target, mut target_kind, mut name, mut tagger) = (None, None, None, None);
    for (key, value) in lines {
        match key {
            b"object" if target.is_none() => target = Some(hex(format, value)?),
            b"type" if target_kind.is_none() => {
                target_kind =
                    Some(Kind::from_name(value).ok_or(invalid("invalid tag target type"))?);
            }
            b"tag" if name.is_none() => name = Some(value.to_vec()),
            b"tagger" if tagger.is_none() => tagger = Some(parse_signature(value)?),
            _ => {}
        }
    }
    Ok(Tag {
        target: target.ok_or(invalid("tag without object"))?,
        target_kind: target_kind.ok_or(invalid("tag without type"))?,
        name: name.ok_or(invalid("tag without name"))?,
        tagger,
        message: message.to_vec(),
    })
}

fn parse_tree(format: ObjectFormat, mut data: &[u8]) -> Result<Vec<TreeEntry>, Error> {
    let error = || invalid("invalid tree entry");
    let mut entries = Vec::new();
    while !data.is_empty() {
        let space = data
            .iter()
            .position(|&byte| byte == b' ')
            .ok_or_else(error)?;
        let nul = space
            + data[space..]
                .iter()
                .position(|&byte| byte == 0)
                .ok_or_else(error)?;
        let mode = core::str::from_utf8(&data[..space])
            .ok()
            .and_then(|mode| u32::from_str_radix(mode, 8).ok())
            .ok_or_else(error)?;
        let end = nul + 1 + format.raw_len();
        let oid = data
            .get(nul + 1..end)
            .and_then(|raw| ObjectId::from_raw(format, raw))
            .ok_or_else(error)?;
        entries.push(TreeEntry {
            mode,
            name: data[space + 1..nul].to_vec(),
            oid,
        });
        data = &data[end..];
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const TREE: &str = "2bfbd6378b73ff610bb4593cab58d200fd0ee64f";
    const PARENT: &str = "28c677f5df3d8f072e45566ed931347ae6325d53";

    #[test]
    fn parses_signed_commit_and_skips_continuation_lines() {
        let data = format!(
            "tree {TREE}\nparent {PARENT}\nauthor Leo K <leo@example.com> 1783070851 +0200\ncommitter GitHub <noreply@github.com> 1783070851 -0130\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n abc\n -----END PGP SIGNATURE-----\n\nRelease v2.0.5\n\nBody\n"
        );
        let Object::Commit(commit) =
            Object::parse(ObjectFormat::Sha1, Kind::Commit, data.as_bytes()).unwrap()
        else {
            panic!("expected commit");
        };
        assert_eq!(commit.tree.to_hex(), TREE);
        assert_eq!(commit.parents, vec![PARENT.parse().unwrap()]);
        assert_eq!(commit.author.name, b"Leo K");
        assert_eq!(commit.author.email, b"leo@example.com");
        assert_eq!(commit.author.time, 1_783_070_851);
        assert_eq!(commit.author.offset_minutes, 120);
        assert_eq!(commit.committer.offset_minutes, -90);
        assert_eq!(commit.encoding, None);
        assert_eq!(commit.message, b"Release v2.0.5\n\nBody\n");
    }

    #[test]
    fn parses_tag_and_tree() {
        let data =
            format!("object {TREE}\ntype tree\ntag snapshot\ntagger A <a@b> 0 +0000\n\nmsg\n");
        let Object::Tag(tag) =
            Object::parse(ObjectFormat::Sha1, Kind::Tag, data.as_bytes()).unwrap()
        else {
            panic!("expected tag");
        };
        assert_eq!(
            (tag.target.to_hex().as_str(), tag.target_kind),
            (TREE, Kind::Tree)
        );
        assert_eq!(
            (tag.name.as_slice(), tag.message.as_slice()),
            (&b"snapshot"[..], &b"msg\n"[..])
        );
        assert_eq!(tag.tagger.unwrap().time, 0);
        let tree_oid: ObjectId = TREE.parse().unwrap();
        let mut tree = b"100644 file\0".to_vec();
        tree.extend_from_slice(tree_oid.as_bytes());
        tree.extend_from_slice(b"40000 dir\0");
        tree.extend_from_slice(tree_oid.as_bytes());
        let Object::Tree(entries) = Object::parse(ObjectFormat::Sha1, Kind::Tree, &tree).unwrap()
        else {
            panic!("expected tree");
        };
        assert_eq!(entries[0].mode, 0o100644);
        assert_eq!(entries[1].name, b"dir");
        assert_eq!(entries[1].oid, tree_oid);
    }

    #[test]
    fn hashes_like_git() {
        assert_eq!(
            object_id(ObjectFormat::Sha1, Kind::Blob, b"").to_hex(),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            object_id(ObjectFormat::Sha256, Kind::Blob, b"").to_hex(),
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"
        );
    }

    #[test]
    fn rejects_incomplete_objects() {
        assert!(
            Object::parse(ObjectFormat::Sha1, Kind::Commit, b"author A <a> 0 +0000\n").is_err()
        );
        let bad_zone = format!("tree {TREE}\nauthor A <a> 0 0200\ncommitter A <a> 0 +0000\n");
        assert!(Object::parse(ObjectFormat::Sha1, Kind::Commit, bad_zone.as_bytes()).is_err());
        assert!(Object::parse(ObjectFormat::Sha1, Kind::Tag, b"type commit\ntag x\n").is_err());
        assert!(Object::parse(ObjectFormat::Sha1, Kind::Tree, b"100644 file\0short").is_err());
    }
}
