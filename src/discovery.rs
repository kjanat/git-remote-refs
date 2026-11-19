use crate::{
    Error, ObjectFormat, RefList, Response,
    http::{self, ADVERTISEMENT},
    pkt::{Packet, Reader, Writer, text},
    refs::parse_v0,
};
use alloc::{collections::BTreeMap, format, string::String};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Discovery {
    V0(RefList),
    V2(Capabilities),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    format: ObjectFormat,
    entries: BTreeMap<String, Option<String>>,
}

impl Capabilities {
    pub const fn object_format(&self) -> ObjectFormat {
        self.format
    }

    pub fn get(&self, key: &str) -> Option<Option<&str>> {
        self.entries.get(key).map(Option::as_deref)
    }

    pub fn supports(&self, command: &str, feature: &str) -> bool {
        matches!(self.get(command), Some(Some(features)) if features.split(' ').any(|name| name == feature))
    }

    pub(crate) fn write_object_format(&self, writer: &mut Writer) -> Result<(), Error> {
        if self.get("object-format").is_some() {
            writer.line(&format!("object-format={}", self.format.name()))?;
        }
        Ok(())
    }
}

pub fn parse_discovery(response: &Response) -> Result<Discovery, Error> {
    parse_body(http::body(response, ADVERTISEMENT)?)
}

pub fn parse_advertisement(body: &[u8]) -> Result<RefList, Error> {
    match parse_body(body)? {
        Discovery::V0(list) => Ok(list),
        Discovery::V2(_) => Err(Error::Protocol("expected a protocol v0 advertisement")),
    }
}

pub(crate) fn parse_body(body: &[u8]) -> Result<Discovery, Error> {
    let mut reader = Reader::new(body);
    if let Packet::Data(line) = reader.peek()?
        && text(line) == b"# service=git-upload-pack"
    {
        reader.read()?;
        if reader.read()? != Packet::Flush {
            return Err(Error::Protocol("expected flush after service announcement"));
        }
    }
    match reader.peek()? {
        Packet::Data(line) if text(line) == b"version 2" => {
            reader.read()?;
            parse_capabilities(&mut reader).map(Discovery::V2)
        }
        Packet::Data(line) if text(line) == b"version 1" => {
            reader.read()?;
            parse_v0(&mut reader).map(Discovery::V0)
        }
        _ => parse_v0(&mut reader).map(Discovery::V0),
    }
}

fn parse_capabilities(reader: &mut Reader) -> Result<Capabilities, Error> {
    let mut capabilities = Capabilities {
        format: ObjectFormat::Sha1,
        entries: BTreeMap::new(),
    };
    while let Some(line) = reader.line()? {
        let line =
            core::str::from_utf8(line).map_err(|_| Error::Protocol("capability is not UTF-8"))?;
        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (key, Some(String::from(value))),
            None => (line, None),
        };
        if key == "object-format" {
            capabilities.format = value
                .as_deref()
                .and_then(|name| ObjectFormat::from_name(name.as_bytes()))
                .ok_or(Error::UnsupportedObjectFormat)?;
        }
        capabilities.entries.insert(key.into(), value);
    }
    if !reader.is_empty() {
        return Err(Error::Protocol("unexpected data after flush"));
    }
    Ok(capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB_V2: &[u8] = b"001e# service=git-upload-pack\n0000000eversion 2\n0013ls-refs=unborn\n0027fetch=shallow wait-for-done filter\n0012server-option\n0017object-format=sha1\n0000";

    #[test]
    fn reads_v2_capabilities_with_or_without_service_line() {
        let Discovery::V2(capabilities) = parse_body(GITHUB_V2).unwrap() else {
            panic!("expected v2");
        };
        assert_eq!(capabilities.object_format(), ObjectFormat::Sha1);
        assert!(capabilities.supports("fetch", "filter"));
        assert!(capabilities.supports("ls-refs", "unborn"));
        assert!(!capabilities.supports("fetch", "sideband-all"));
        assert!(!capabilities.supports("server-option", "x"));
        assert_eq!(capabilities.get("server-option"), Some(None));
        let bare = &GITHUB_V2[34..];
        assert_eq!(parse_body(bare).unwrap(), Discovery::V2(capabilities));
    }

    #[test]
    fn reads_v0_and_version_1_advertisements() {
        let record = b"003d1111111111111111111111111111111111111111 refs/heads/main\n0000";
        let Discovery::V0(list) = parse_body(record).unwrap() else {
            panic!("expected v0");
        };
        assert!(list.get("refs/heads/main").is_some());
        let mut versioned = b"000eversion 1\n".to_vec();
        versioned.extend_from_slice(record);
        assert_eq!(parse_body(&versioned).unwrap(), Discovery::V0(list.clone()));
        assert_eq!(parse_advertisement(record), Ok(list));
        assert!(parse_advertisement(GITHUB_V2).is_err());
    }

    #[test]
    fn rejects_bad_service_framing_and_formats() {
        assert!(parse_body(b"001e# service=git-upload-pack\n000eversion 2\n0000").is_err());
        assert_eq!(
            parse_body(b"000eversion 2\n0016object-format=md5\n0000"),
            Err(Error::UnsupportedObjectFormat)
        );
        assert!(parse_body(b"000eversion 2\n00000000").is_err());
    }
}
