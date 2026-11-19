use crate::Error;
use alloc::string::String;
use core::{fmt, str::FromStr};
use sha1::Digest;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}

impl ObjectFormat {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }

    pub const fn raw_len(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
        }
    }

    pub const fn hex_len(self) -> usize {
        self.raw_len() * 2
    }

    pub fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"sha1" => Some(Self::Sha1),
            b"sha256" => Some(Self::Sha256),
            _ => None,
        }
    }
}

pub(crate) enum Hasher {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    pub(crate) fn new(format: ObjectFormat) -> Self {
        match format {
            ObjectFormat::Sha1 => Self::Sha1(sha1::Sha1::new()),
            ObjectFormat::Sha256 => Self::Sha256(sha2::Sha256::new()),
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha1(hasher) => hasher.update(data),
            Self::Sha256(hasher) => hasher.update(data),
        }
    }

    pub(crate) fn finish(self) -> ObjectId {
        let mut bytes = [0; 32];
        let format = match self {
            Self::Sha1(hasher) => {
                bytes[..20].copy_from_slice(&hasher.finalize());
                ObjectFormat::Sha1
            }
            Self::Sha256(hasher) => {
                bytes.copy_from_slice(&hasher.finalize());
                ObjectFormat::Sha256
            }
        };
        ObjectId { format, bytes }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId {
    format: ObjectFormat,
    bytes: [u8; 32],
}

impl ObjectId {
    pub fn from_raw(format: ObjectFormat, raw: &[u8]) -> Option<Self> {
        if raw.len() != format.raw_len() {
            return None;
        }
        let mut bytes = [0; 32];
        bytes[..raw.len()].copy_from_slice(raw);
        Some(Self { format, bytes })
    }

    pub fn from_hex(hex: &[u8]) -> Option<Self> {
        match hex.len() {
            40 => Self::parse_hex(ObjectFormat::Sha1, hex),
            64 => Self::parse_hex(ObjectFormat::Sha256, hex),
            _ => None,
        }
    }

    pub(crate) fn parse_hex(format: ObjectFormat, hex: &[u8]) -> Option<Self> {
        if hex.len() != format.hex_len() {
            return None;
        }
        let mut bytes = [0; 32];
        for (byte, [high, low]) in bytes.iter_mut().zip(hex.as_chunks::<2>().0) {
            let high = char::from(*high).to_digit(16)?;
            let low = char::from(*low).to_digit(16)?;
            *byte = u8::try_from(high << 4 | low).ok()?;
        }
        Some(Self { format, bytes })
    }

    pub const fn format(&self) -> ObjectFormat {
        self.format
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.format.raw_len()]
    }

    pub fn is_null(&self) -> bool {
        self.as_bytes().iter().all(|&byte| byte == 0)
    }

    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(self.format.hex_len());
        for &byte in self.as_bytes() {
            hex.push(char::from(DIGITS[usize::from(byte >> 4)]));
            hex.push(char::from(DIGITS[usize::from(byte & 15)]));
        }
        hex
    }
}

impl FromStr for ObjectId {
    type Err = Error;

    fn from_str(hex: &str) -> Result<Self, Error> {
        Self::from_hex(hex.as_bytes()).ok_or(Error::InvalidObjectId)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_both_formats() {
        let sha1 = "22d081ff2d3a40755e97629de92e3bcbfa7cf2ed";
        let oid: ObjectId = sha1.parse().unwrap();
        assert_eq!(oid.format(), ObjectFormat::Sha1);
        assert_eq!(oid.to_hex(), sha1);
        let upper: ObjectId = sha1.to_ascii_uppercase().parse().unwrap();
        assert_eq!(upper, oid);
        let sha256 = "a".repeat(64);
        assert_eq!(
            sha256.parse::<ObjectId>().unwrap().format(),
            ObjectFormat::Sha256
        );
    }

    #[test]
    fn rejects_abbreviated_and_non_hex_ids() {
        assert_eq!("22d081f".parse::<ObjectId>(), Err(Error::InvalidObjectId));
        assert_eq!(
            "g".repeat(40).parse::<ObjectId>(),
            Err(Error::InvalidObjectId)
        );
        assert!(ObjectId::parse_hex(ObjectFormat::Sha256, &[b'0'; 40]).is_none());
        assert!(ObjectId::from_raw(ObjectFormat::Sha1, &[0; 32]).is_none());
    }
}
