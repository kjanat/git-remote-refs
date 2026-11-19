use crate::{
    Capabilities, Error, ObjectFormat, ObjectId, Remote, Request, Response,
    http::{self, RESULT},
    pkt::{Reader, Writer},
};
use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub oid: ObjectId,
    pub peeled: Option<ObjectId>,
    pub symref_target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefList {
    pub format: ObjectFormat,
    pub refs: BTreeMap<String, Ref>,
    pub unborn: BTreeMap<String, String>,
    pub prefixes: Option<Vec<String>>,
}

impl RefList {
    fn new(format: ObjectFormat, prefixes: &[&str]) -> Self {
        Self {
            format,
            refs: BTreeMap::new(),
            unborn: BTreeMap::new(),
            prefixes: (!prefixes.is_empty())
                .then(|| prefixes.iter().map(ToString::to_string).collect()),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Ref> {
        self.refs.get(name)
    }

    pub fn is_complete(&self) -> bool {
        self.prefixes.is_none()
    }

    pub fn covers(&self, name: &str) -> bool {
        self.prefixes.as_ref().is_none_or(|prefixes| {
            prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str()))
        })
    }

    pub fn restrict(mut self, prefixes: &[&str]) -> Result<Self, Error> {
        if prefixes.is_empty() {
            return Ok(self);
        }
        for prefix in prefixes {
            check_prefix(prefix)?;
        }
        let keep = |name: &String| prefixes.iter().any(|prefix| name.starts_with(prefix));
        self.refs.retain(|name, _| keep(name));
        self.unborn.retain(|name, _| keep(name));
        let mut narrowed: Vec<String> = prefixes.iter().map(ToString::to_string).collect();
        if let Some(previous) = self.prefixes.take() {
            narrowed.retain(|prefix| {
                previous
                    .iter()
                    .any(|outer| prefix.starts_with(outer.as_str()))
            });
        }
        self.prefixes = Some(narrowed);
        Ok(self)
    }
}

pub fn check_ref_name(name: &str) -> Result<(), Error> {
    let forbidden = |byte: u8| {
        byte < 0x20
            || byte == 0x7f
            || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
    };
    let invalid = name.is_empty()
        || name == "@"
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || name.bytes().any(forbidden)
        || name
            .split('/')
            .any(|component| component.starts_with('.') || component.ends_with(".lock"));
    if invalid {
        Err(Error::InvalidRefName)
    } else {
        Ok(())
    }
}

pub(crate) fn check_prefix(prefix: &str) -> Result<(), Error> {
    if prefix.is_empty() || prefix.bytes().any(|byte| byte <= b' ' || byte == 0x7f) {
        return Err(Error::InvalidPrefix);
    }
    Ok(())
}

fn utf8(bytes: &[u8]) -> Result<String, Error> {
    core::str::from_utf8(bytes)
        .map(Into::into)
        .map_err(|_| Error::Protocol("ref name is not UTF-8"))
}

fn oid(format: ObjectFormat, hex: &[u8]) -> Result<ObjectId, Error> {
    ObjectId::parse_hex(format, hex).ok_or(Error::Protocol("invalid object ID"))
}

pub(crate) fn parse_v0(reader: &mut Reader) -> Result<RefList, Error> {
    let mut list = RefList::new(ObjectFormat::Sha1, &[]);
    let mut symrefs = Vec::new();
    let mut first = true;
    while let Some(line) = reader.line()? {
        let (record, capabilities) = match line.iter().position(|&byte| byte == 0) {
            Some(nul) => (&line[..nul], Some(&line[nul + 1..])),
            None => (line, None),
        };
        if let Some(capabilities) = capabilities {
            if !first {
                return Err(Error::Protocol("unexpected capability list"));
            }
            for capability in capabilities.split(|&byte| byte == b' ') {
                if let Some(pair) = capability.strip_prefix(b"symref=") {
                    let colon = pair
                        .iter()
                        .position(|&byte| byte == b':')
                        .ok_or(Error::Protocol("invalid symref capability"))?;
                    symrefs.push((utf8(&pair[..colon])?, utf8(&pair[colon + 1..])?));
                }
                if let Some(name) = capability.strip_prefix(b"object-format=") {
                    list.format =
                        ObjectFormat::from_name(name).ok_or(Error::UnsupportedObjectFormat)?;
                }
            }
        }
        first = false;
        let (hex, name) = record
            .split_at_checked(list.format.hex_len())
            .ok_or(Error::Protocol("short ref record"))?;
        let name = name
            .strip_prefix(b" ")
            .ok_or(Error::Protocol("missing ref name"))?;
        let id = oid(list.format, hex)?;
        if name == b"capabilities^{}" {
            if !id.is_null() {
                return Err(Error::Protocol(
                    "capabilities^{} must use the null object ID",
                ));
            }
            continue;
        }
        let name = utf8(name)?;
        if name.is_empty() {
            return Err(Error::Protocol("missing ref name"));
        }
        match name.strip_suffix("^{}") {
            Some(base) => {
                list.refs
                    .get_mut(base)
                    .ok_or(Error::Protocol("peeled entry without its ref"))?
                    .peeled = Some(id);
            }
            None => {
                list.refs.insert(
                    name,
                    Ref {
                        oid: id,
                        peeled: None,
                        symref_target: None,
                    },
                );
            }
        }
    }
    if !reader.is_empty() {
        return Err(Error::Protocol("unexpected data after flush"));
    }
    for (name, target) in symrefs {
        match list.refs.get_mut(&name) {
            Some(entry) => entry.symref_target = Some(target),
            None => {
                list.unborn.insert(name, target);
            }
        }
    }
    Ok(list)
}

impl Remote {
    pub fn ls_refs_request(
        &self,
        capabilities: &Capabilities,
        prefixes: &[&str],
    ) -> Result<Request, Error> {
        if capabilities.get("ls-refs").is_none() {
            return Err(Error::UnsupportedFeature("ls-refs"));
        }
        let mut writer = Writer::default();
        writer.line("command=ls-refs")?;
        capabilities.write_object_format(&mut writer)?;
        writer.delim();
        writer.line("peel")?;
        writer.line("symrefs")?;
        if capabilities.supports("ls-refs", "unborn") {
            writer.line("unborn")?;
        }
        for prefix in prefixes {
            check_prefix(prefix)?;
            writer.line(&format!("ref-prefix {prefix}"))?;
        }
        writer.flush();
        Ok(self.post(writer.finish()))
    }
}

pub fn parse_ls_refs(
    capabilities: &Capabilities,
    response: &Response,
    prefixes: &[&str],
) -> Result<RefList, Error> {
    let format = capabilities.object_format();
    let mut reader = Reader::new(http::body(response, RESULT)?);
    let mut list = RefList::new(format, prefixes);
    while let Some(line) = reader.line()? {
        let mut fields = line.split(|&byte| byte == b' ');
        let (Some(first), Some(name)) = (fields.next(), fields.next()) else {
            return Err(Error::Protocol("short ref record"));
        };
        let name = utf8(name)?;
        let mut peeled = None;
        let mut symref_target = None;
        for attribute in fields {
            if let Some(target) = attribute.strip_prefix(b"symref-target:") {
                symref_target = Some(utf8(target)?);
            } else if let Some(hex) = attribute.strip_prefix(b"peeled:") {
                peeled = Some(oid(format, hex)?);
            }
        }
        if first == b"unborn" {
            let target =
                symref_target.ok_or(Error::Protocol("unborn ref without symref target"))?;
            list.unborn.insert(name, target);
        } else {
            list.refs.insert(
                name,
                Ref {
                    oid: oid(format, first)?,
                    peeled,
                    symref_target,
                },
            );
        }
    }
    if !reader.is_empty() {
        return Err(Error::Protocol("unexpected data after flush"));
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pkt(line: &[u8]) -> Vec<u8> {
        let mut out = format!("{:04x}", line.len() + 4).into_bytes();
        out.extend_from_slice(line);
        out
    }

    fn v0(lines: &[&[u8]]) -> Result<RefList, Error> {
        let mut input: Vec<u8> = lines.iter().flat_map(|line| pkt(line)).collect();
        input.extend_from_slice(b"0000");
        parse_v0(&mut Reader::new(&input))
    }

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn v0_attaches_peeled_entries_and_symrefs() {
        let first = format!("{A} HEAD\0symref=HEAD:refs/heads/main agent=git/x\n");
        let main = format!("{A} refs/heads/main\n");
        let tag = format!("{B} refs/tags/v1\n");
        let peeled = format!("{A} refs/tags/v1^{{}}\n");
        let list = v0(&[
            first.as_bytes(),
            main.as_bytes(),
            tag.as_bytes(),
            peeled.as_bytes(),
        ])
        .unwrap();
        assert_eq!(
            list.get("HEAD").unwrap().symref_target.as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(
            list.get("refs/tags/v1").unwrap().peeled,
            Some(A.parse().unwrap())
        );
        assert_eq!(list.refs.len(), 3);
        assert!(list.is_complete());
    }

    #[test]
    fn v0_empty_repository_reports_unborn_head() {
        let record = format!(
            "{} capabilities^{{}}\0symref=HEAD:refs/heads/main object-format=sha1\n",
            "0".repeat(40)
        );
        let list = v0(&[record.as_bytes()]).unwrap();
        assert!(list.refs.is_empty());
        assert_eq!(
            list.unborn.get("HEAD").map(String::as_str),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn v0_reads_sha256_ids_after_object_format() {
        let oid = "ab".repeat(32);
        let record = format!("{oid} refs/heads/main\0object-format=sha256\n");
        let list = v0(&[record.as_bytes()]).unwrap();
        assert_eq!(list.format, ObjectFormat::Sha256);
        assert_eq!(list.get("refs/heads/main").unwrap().oid.to_hex(), oid);
    }

    #[test]
    fn v0_rejects_malformed_records() {
        let late = format!("{A} refs/heads/b\0agent=x\n");
        let first = format!("{A} refs/heads/a\n");
        assert!(v0(&[first.as_bytes(), late.as_bytes()]).is_err());
        let orphan = format!("{A} refs/tags/x^{{}}\n");
        assert!(v0(&[orphan.as_bytes()]).is_err());
        let fake = format!("{A} capabilities^{{}}\n");
        assert!(v0(&[fake.as_bytes()]).is_err());
        assert!(v0(&[b"1234 refs/heads/a\n"]).is_err());
        let unknown = format!("{A} HEAD\0object-format=md5\n");
        assert_eq!(
            v0(&[unknown.as_bytes()]),
            Err(Error::UnsupportedObjectFormat)
        );
        let mut trailing = pkt(format!("{A} HEAD\n").as_bytes());
        trailing.extend_from_slice(b"00000000");
        assert!(parse_v0(&mut Reader::new(&trailing)).is_err());
    }

    #[test]
    fn ref_names_follow_check_ref_format() {
        for name in [
            "HEAD",
            "refs/heads/main",
            "refs/tags/v1.0",
            "heads/x",
            "a@b",
            "unicodé",
        ] {
            assert_eq!(check_ref_name(name), Ok(()), "{name}");
        }
        for name in [
            "",
            "@",
            "/a",
            "a/",
            "a.",
            "a..b",
            "a//b",
            "a@{1}",
            "a b",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[b",
            "a\\b",
            "a/.b",
            "a.lock",
            "a/b.lock/c",
            "a\u{7f}",
            "a\nb",
        ] {
            assert_eq!(check_ref_name(name), Err(Error::InvalidRefName), "{name:?}");
        }
    }

    #[test]
    fn restrict_filters_and_records_coverage() {
        let main = format!("{A} refs/heads/main\n");
        let tag = format!("{B} refs/tags/v1\n");
        let list = v0(&[main.as_bytes(), tag.as_bytes()]).unwrap();
        let tags = list.clone().restrict(&["refs/tags/"]).unwrap();
        assert_eq!(tags.refs.keys().collect::<Vec<_>>(), vec!["refs/tags/v1"]);
        assert!(tags.covers("refs/tags/missing"));
        assert!(!tags.covers("refs/heads/main"));
        let narrower = tags
            .clone()
            .restrict(&["refs/tags/v", "refs/heads/"])
            .unwrap();
        assert_eq!(narrower.prefixes, Some(vec!["refs/tags/v".into()]));
        assert_eq!(list.clone().restrict(&[]).unwrap(), list);
        assert_eq!(list.restrict(&["bad prefix"]), Err(Error::InvalidPrefix));
    }
}
