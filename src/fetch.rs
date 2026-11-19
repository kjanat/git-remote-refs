use crate::{
    Capabilities, Error, Limits, Object, ObjectId, Remote, Request, Response,
    http::{self, RESULT},
    pack,
    pkt::{Packet, Reader, Writer, text},
};
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};

impl Remote {
    pub fn fetch_request(
        &self,
        capabilities: &Capabilities,
        wants: &[ObjectId],
    ) -> Result<Request, Error> {
        if capabilities.get("fetch").is_none() {
            return Err(Error::UnsupportedFeature("fetch"));
        }
        if !capabilities.supports("fetch", "shallow") {
            return Err(Error::UnsupportedFeature("shallow fetch"));
        }
        if wants.is_empty() {
            return Err(Error::InvalidObjectId);
        }
        if wants
            .iter()
            .any(|want| want.format() != capabilities.object_format())
        {
            return Err(Error::UnsupportedObjectFormat);
        }
        let mut writer = Writer::default();
        writer.line("command=fetch")?;
        capabilities.write_object_format(&mut writer)?;
        writer.delim();
        writer.line("ofs-delta")?;
        writer.line("no-progress")?;
        writer.line("deepen 1")?;
        if capabilities.supports("fetch", "filter") {
            writer.line("filter tree:0")?;
        }
        for want in wants {
            writer.line(&format!("want {want}"))?;
        }
        writer.line("done")?;
        writer.flush();
        Ok(self.post(writer.finish()))
    }
}

fn remote(message: &[u8]) -> Error {
    Error::Remote(String::from_utf8_lossy(text(message)).into_owned())
}

fn packfile(body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut reader = Reader::new(body);
    loop {
        let header = match reader.read()? {
            Packet::Data(line) => text(line),
            Packet::Flush | Packet::ResponseEnd => {
                return Err(Error::Protocol("response has no packfile"));
            }
            Packet::Delim => return Err(Error::Protocol("unexpected packet")),
        };
        if let Some(message) = header.strip_prefix(b"ERR ") {
            return Err(remote(message));
        }
        if header != b"packfile" {
            loop {
                match reader.read()? {
                    Packet::Data(_) => {}
                    Packet::Delim => break,
                    Packet::Flush | Packet::ResponseEnd => {
                        return Err(Error::Protocol("response has no packfile"));
                    }
                }
            }
            continue;
        }
        let mut pack = Vec::new();
        loop {
            match reader.read()? {
                Packet::Data(data) => match data.split_first() {
                    Some((1, chunk)) => pack.extend_from_slice(chunk),
                    Some((2, _)) => {}
                    Some((3, message)) => return Err(remote(message)),
                    _ => return Err(Error::Protocol("invalid sideband packet")),
                },
                Packet::Flush => break,
                Packet::Delim | Packet::ResponseEnd => {
                    return Err(Error::Protocol("unexpected packet"));
                }
            }
        }
        if !reader.is_empty() {
            return Err(Error::Protocol("unexpected data after packfile"));
        }
        return Ok(pack);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub wanted: Vec<(ObjectId, Object)>,
    pub objects: BTreeMap<ObjectId, Object>,
}

pub fn parse_fetch(
    capabilities: &Capabilities,
    response: &Response,
    wants: &[ObjectId],
    limits: &Limits,
) -> Result<Fetched, Error> {
    let format = capabilities.object_format();
    let pack = packfile(http::body(response, RESULT)?)?;
    let mut objects = BTreeMap::new();
    for (id, (kind, data)) in pack::parse(format, &pack, limits)? {
        objects.insert(id, Object::parse(format, kind, &data)?);
    }
    let wanted = wants
        .iter()
        .map(|want| match objects.get(want) {
            Some(object) => Ok((*want, object.clone())),
            None => Err(Error::MissingObject(*want)),
        })
        .collect::<Result<_, _>>()?;
    Ok(Fetched { wanted, objects })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkt(data: &[u8]) -> Vec<u8> {
        let mut out = format!("{:04x}", data.len() + 4).into_bytes();
        out.extend_from_slice(data);
        out
    }

    fn capabilities(lines: &[&str]) -> Capabilities {
        let mut body = pkt(b"version 2\n");
        for line in lines {
            body.extend(pkt(format!("{line}\n").as_bytes()));
        }
        body.extend_from_slice(b"0000");
        match crate::discovery::parse_body(&body) {
            Ok(crate::Discovery::V2(capabilities)) => capabilities,
            other => panic!("expected capabilities, got {other:?}"),
        }
    }

    #[test]
    fn fetch_request_requires_shallow_and_uses_filter_when_offered() {
        let remote = Remote::new("https://example.com/repo.git").unwrap();
        let oid: ObjectId = "1111111111111111111111111111111111111111".parse().unwrap();
        let full = capabilities(&["fetch=shallow filter", "object-format=sha1"]);
        let request = remote.fetch_request(&full, &[oid]).unwrap();
        assert_eq!(request.url, "https://example.com/repo.git/git-upload-pack");
        let body = String::from_utf8(request.body).unwrap();
        assert_eq!(
            body,
            "0012command=fetch\n0017object-format=sha1\n0001000eofs-delta\n0010no-progress\n000ddeepen 1\n0012filter tree:0\n0032want 1111111111111111111111111111111111111111\n0009done\n0000"
        );
        let plain = remote
            .fetch_request(&capabilities(&["fetch=shallow"]), &[oid])
            .unwrap();
        let plain = String::from_utf8(plain.body).unwrap();
        assert!(!plain.contains("filter") && !plain.contains("object-format"));
        assert_eq!(
            remote.fetch_request(&capabilities(&["fetch=filter"]), &[oid]),
            Err(Error::UnsupportedFeature("shallow fetch"))
        );
        assert_eq!(
            remote.fetch_request(&capabilities(&["ls-refs"]), &[oid]),
            Err(Error::UnsupportedFeature("fetch"))
        );
        assert_eq!(
            remote.fetch_request(&full, &[]),
            Err(Error::InvalidObjectId)
        );
        assert_eq!(
            remote.ls_refs_request(&capabilities(&["fetch=shallow"]), &[]),
            Err(Error::UnsupportedFeature("ls-refs"))
        );
        let listing = remote
            .ls_refs_request(&capabilities(&["ls-refs"]), &["refs/tags/"])
            .unwrap();
        assert_eq!(
            String::from_utf8(listing.body).unwrap(),
            "0014command=ls-refs\n00010009peel\n000csymrefs\n001aref-prefix refs/tags/\n0000"
        );
    }

    #[test]
    fn demultiplexes_sideband_after_other_sections() {
        let mut body = pkt(b"shallow-info\n");
        body.extend(pkt(b"shallow 1111111111111111111111111111111111111111\n"));
        body.extend_from_slice(b"0001");
        body.extend(pkt(b"packfile\n"));
        body.extend(pkt(b"\x01PA"));
        body.extend(pkt(b"\x02progress\n"));
        body.extend(pkt(b"\x01CK"));
        body.extend_from_slice(b"0000");
        assert_eq!(packfile(&body), Ok(b"PACK".to_vec()));
    }

    #[test]
    fn surfaces_server_errors() {
        assert_eq!(
            packfile(&pkt(b"ERR upload-pack: not our ref 1111\n")),
            Err(Error::Remote("upload-pack: not our ref 1111".into()))
        );
        let mut body = pkt(b"packfile\n");
        body.extend(pkt(b"\x03fatal: oops\n"));
        assert_eq!(packfile(&body), Err(Error::Remote("fatal: oops".into())));
        assert!(packfile(b"0000").is_err());
        let mut bad = pkt(b"packfile\n");
        bad.extend(pkt(b"\x09x"));
        assert!(packfile(&bad).is_err());
        let mut trailing = pkt(b"packfile\n");
        trailing.extend_from_slice(b"00000000");
        assert!(packfile(&trailing).is_err());
        assert_eq!(
            packfile(&[]),
            Err(Error::Protocol("truncated packet header"))
        );
    }
}
