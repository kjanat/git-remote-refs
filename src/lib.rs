//! Git smart-HTTP v0 upload-pack reference discovery.
use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Advertisement {
    /// Ref names and their advertised object IDs, including peeled tag entries.
    pub refs: BTreeMap<Vec<u8>, String>,
    pub symrefs: BTreeMap<Vec<u8>, Vec<u8>>,
}

fn packet<'a>(input: &mut &'a [u8]) -> Result<Option<&'a [u8]>, &'static str> {
    let header = input.get(..4).ok_or("truncated packet header")?;
    if !header.iter().all(u8::is_ascii_hexdigit) {
        return Err("invalid packet length");
    }
    let size = usize::from_str_radix(std::str::from_utf8(header).unwrap(), 16).unwrap();
    if size == 0 {
        *input = &input[4..];
        return Ok(None);
    }
    if !(4..=65520).contains(&size) {
        return Err("unsupported packet length");
    }
    let payload = input.get(4..size).ok_or("truncated packet")?;
    *input = &input[size..];
    Ok(Some(payload.strip_suffix(b"\n").unwrap_or(payload)))
}

pub fn parse_advertisement(mut input: &[u8]) -> Result<Advertisement, &'static str> {
    if packet(&mut input)? != Some(b"# service=git-upload-pack") || packet(&mut input)?.is_some() {
        return Err("expected upload-pack service announcement");
    }
    let mut result = Advertisement::default();
    let mut first = true;
    let mut oid_len = 40;
    while let Some(line) = packet(&mut input)? {
        let mut fields = line.splitn(2, |&b| b == 0);
        let record = fields.next().unwrap();
        if let Some(capabilities) = fields.next() {
            if !first {
                return Err("unexpected capability list");
            }
            for cap in capabilities.split(|&b| b == b' ') {
                if let Some(pair) = cap.strip_prefix(b"symref=") {
                    let colon = pair
                        .iter()
                        .position(|&b| b == b':')
                        .ok_or("invalid symref")?;
                    result
                        .symrefs
                        .insert(pair[..colon].to_vec(), pair[colon + 1..].to_vec());
                }
                if let Some(format) = cap.strip_prefix(b"object-format=") {
                    oid_len = match format {
                        b"sha1" => 40,
                        b"sha256" => 64,
                        _ => return Err("unsupported object format"),
                    };
                }
            }
        }
        first = false;
        let (oid, name) = record.split_at_checked(oid_len).ok_or("short ref record")?;
        let name = name.strip_prefix(b" ").ok_or("missing ref name")?;
        if !oid.iter().all(u8::is_ascii_hexdigit) || name.is_empty() {
            return Err("invalid ref record");
        }
        if name == b"capabilities^{}" && oid.iter().all(|&b| b == b'0') {
            continue;
        }
        result
            .refs
            .insert(name.to_vec(), std::str::from_utf8(oid).unwrap().to_owned());
    }
    if !input.is_empty() {
        return Err("unexpected data after flush");
    }
    Ok(result)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use js_sys::{Map, Object, Reflect};
    use wasm_bindgen::prelude::*;

    fn name(bytes: &[u8]) -> Result<JsValue, JsError> {
        Ok(std::str::from_utf8(bytes)?.into())
    }

    #[wasm_bindgen(typescript_custom_section)]
    const ADVERTISEMENT: &str = r#"
export interface Advertisement {
    /** Advertised ref name to object ID, including peeled tag entries. */
    refs: Map<string, string>;
    /** Symbolic ref name to target ref name. */
    symrefs: Map<string, string>;
}
"#;

    #[wasm_bindgen(js_name = parseAdvertisement, unchecked_return_type = "Advertisement")]
    pub fn parse(bytes: &[u8]) -> Result<Object, JsError> {
        let parsed = super::parse_advertisement(bytes).map_err(JsError::new)?;
        let refs = Map::new();
        for (key, oid) in parsed.refs {
            refs.set(&name(&key)?, &oid.into());
        }
        let symrefs = Map::new();
        for (key, target) in parsed.symrefs {
            symrefs.set(&name(&key)?, &name(&target)?);
        }
        let obj = Object::new();
        Reflect::set(&obj, &"refs".into(), &refs).unwrap();
        Reflect::set(&obj, &"symrefs".into(), &symrefs).unwrap();
        Ok(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_partial_packets_and_missing_flush() {
        assert!(parse_advertisement(b"001e# service=git-upload-pack\n0000").is_err());
        assert!(packet(&mut &b"0008abc"[..]).is_err());
    }

    #[test]
    fn empty_repository_has_no_synthetic_ref() {
        let record =
            b"0000000000000000000000000000000000000000 capabilities^{}\0object-format=sha1\n";
        let mut input = b"001e# service=git-upload-pack\n0000".to_vec();
        input.extend(format!("{:04x}", record.len() + 4).bytes());
        input.extend(record);
        input.extend(b"0000");
        assert_eq!(
            parse_advertisement(&input).unwrap(),
            Advertisement::default()
        );
    }
}
