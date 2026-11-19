use crate::Error;
use alloc::vec::Vec;

const MAX_PACKET: usize = 65520;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Packet<'a> {
    Flush,
    Delim,
    ResponseEnd,
    Data(&'a [u8]),
}

pub(crate) struct Reader<'a> {
    input: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(input: &'a [u8]) -> Self {
        Self { input }
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    pub(crate) fn peek(&self) -> Result<Packet<'a>, Error> {
        Reader { input: self.input }.read()
    }

    pub(crate) fn read(&mut self) -> Result<Packet<'a>, Error> {
        let header = self
            .input
            .get(..4)
            .ok_or(Error::Protocol("truncated packet header"))?;
        let mut size = 0;
        for &byte in header {
            let digit = char::from(byte)
                .to_digit(16)
                .ok_or(Error::Protocol("invalid packet length"))?;
            size = size * 16 + digit as usize;
        }
        let (packet, consumed) = match size {
            0 => (Packet::Flush, 4),
            1 => (Packet::Delim, 4),
            2 => (Packet::ResponseEnd, 4),
            4..=MAX_PACKET => {
                let payload = self
                    .input
                    .get(4..size)
                    .ok_or(Error::Protocol("truncated packet"))?;
                (Packet::Data(payload), size)
            }
            _ => return Err(Error::Protocol("invalid packet length")),
        };
        self.input = &self.input[consumed..];
        Ok(packet)
    }

    pub(crate) fn line(&mut self) -> Result<Option<&'a [u8]>, Error> {
        match self.read()? {
            Packet::Data(data) => Ok(Some(text(data))),
            Packet::Flush => Ok(None),
            Packet::Delim | Packet::ResponseEnd => Err(Error::Protocol("unexpected packet")),
        }
    }
}

pub(crate) fn text(data: &[u8]) -> &[u8] {
    data.strip_suffix(b"\n").unwrap_or(data)
}

#[derive(Default)]
pub(crate) struct Writer {
    output: Vec<u8>,
}

impl Writer {
    pub(crate) fn line(&mut self, line: &str) -> Result<(), Error> {
        let size = line.len() + 5;
        if size > MAX_PACKET {
            return Err(Error::LimitExceeded("packet length"));
        }
        self.output
            .extend_from_slice(alloc::format!("{size:04x}{line}\n").as_bytes());
        Ok(())
    }

    pub(crate) fn delim(&mut self) {
        self.output.extend_from_slice(b"0001");
    }

    pub(crate) fn flush(&mut self) {
        self.output.extend_from_slice(b"0000");
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_special_and_data_packets() {
        let mut reader = Reader::new(b"0000000100020009done\n0004");
        assert_eq!(reader.read(), Ok(Packet::Flush));
        assert_eq!(reader.read(), Ok(Packet::Delim));
        assert_eq!(reader.read(), Ok(Packet::ResponseEnd));
        assert_eq!(reader.peek(), Ok(Packet::Data(b"done\n")));
        assert_eq!(reader.line(), Ok(Some(&b"done"[..])));
        assert_eq!(reader.read(), Ok(Packet::Data(b"")));
        assert!(reader.is_empty());
    }

    #[test]
    fn rejects_malformed_packets() {
        for input in [&b"00"[..], b"0003", b"zzzz", b"0008abc", b"fff1"] {
            assert!(Reader::new(input).read().is_err(), "{input:?}");
        }
        assert!(Reader::new(b"0001").line().is_err());
    }

    #[test]
    fn writes_lines_with_length_prefix() {
        let mut writer = Writer::default();
        writer.line("command=ls-refs").unwrap();
        writer.delim();
        writer.flush();
        assert_eq!(writer.finish(), b"0014command=ls-refs\n00010000");
        let long = "x".repeat(MAX_PACKET);
        assert_eq!(
            Writer::default().line(&long),
            Err(Error::LimitExceeded("packet length"))
        );
    }
}
