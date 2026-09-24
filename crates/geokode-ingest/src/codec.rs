use std::io::{self, Read, Write};

pub struct Encoder<W: Write> {
    out: W,
}

impl<W: Write> Encoder<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }

    pub fn unsigned(&mut self, mut value: u64) -> io::Result<()> {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                return self.out.write_all(&[byte]);
            }
            self.out.write_all(&[byte | 0x80])?;
        }
    }

    pub fn signed(&mut self, value: i64) -> io::Result<()> {
        self.unsigned(((value << 1) ^ (value >> 63)) as u64)
    }

    pub fn text(&mut self, value: &str) -> io::Result<()> {
        self.unsigned(value.len() as u64)?;
        self.out.write_all(value.as_bytes())
    }

    pub fn optional_text(&mut self, value: Option<&str>) -> io::Result<()> {
        match value {
            Some(text) => {
                self.unsigned(1)?;
                self.text(text)
            }
            None => self.unsigned(0),
        }
    }

    pub fn texts(&mut self, values: &[String]) -> io::Result<()> {
        self.unsigned(values.len() as u64)?;
        values.iter().try_for_each(|value| self.text(value))
    }

    // delta coding keeps runs of nearby node ids to a byte or two each
    pub fn ids(&mut self, ids: &[i64]) -> io::Result<()> {
        self.unsigned(ids.len() as u64)?;
        let mut previous = 0i64;
        for id in ids {
            self.signed(id.wrapping_sub(previous))?;
            previous = *id;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.out.flush()
    }
}

pub struct Decoder<R: Read> {
    input: R,
}

fn unzigzag(raw: u64) -> i64 {
    ((raw >> 1) as i64) ^ -((raw & 1) as i64)
}

impl<R: Read> Decoder<R> {
    pub fn new(input: R) -> Self {
        Self { input }
    }

    // None at a clean end of input, where the next record would start
    pub fn next_unsigned(&mut self) -> io::Result<Option<u64>> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let mut byte = [0u8; 1];
            if self.input.read(&mut byte)? == 0 {
                if shift == 0 {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "scratch file ends inside a number",
                ));
            }
            value |= u64::from(byte[0] & 0x7f) << shift;
            if byte[0] & 0x80 == 0 {
                return Ok(Some(value));
            }
            shift += 7;
        }
    }

    pub fn unsigned(&mut self) -> io::Result<u64> {
        self.next_unsigned()?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "scratch file ends early"))
    }

    pub fn signed(&mut self) -> io::Result<i64> {
        self.unsigned().map(unzigzag)
    }

    pub fn next_signed(&mut self) -> io::Result<Option<i64>> {
        Ok(self.next_unsigned()?.map(unzigzag))
    }

    pub fn text(&mut self) -> io::Result<String> {
        let length = self.unsigned()? as usize;
        let mut bytes = vec![0u8; length];
        self.input.read_exact(&mut bytes)?;
        String::from_utf8(bytes).map_err(io::Error::other)
    }

    pub fn optional_text(&mut self) -> io::Result<Option<String>> {
        match self.unsigned()? {
            0 => Ok(None),
            _ => self.text().map(Some),
        }
    }

    pub fn texts(&mut self) -> io::Result<Vec<String>> {
        let count = self.unsigned()?;
        (0..count).map(|_| self.text()).collect()
    }

    pub fn ids(&mut self) -> io::Result<Vec<i64>> {
        let count = self.unsigned()? as usize;
        let mut ids = Vec::with_capacity(count);
        let mut previous = 0i64;
        for _ in 0..count {
            previous = previous.wrapping_add(self.signed()?);
            ids.push(previous);
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes);
        encoder.unsigned(300).unwrap();
        encoder.signed(-5_000_000_000).unwrap();
        encoder.optional_text(Some("Zürich")).unwrap();
        encoder.optional_text(None).unwrap();
        encoder.ids(&[10, 12, 11, 9_000_000_000]).unwrap();
        encoder.finish().unwrap();

        let mut decoder = Decoder::new(bytes.as_slice());
        assert_eq!(decoder.unsigned().unwrap(), 300);
        assert_eq!(decoder.signed().unwrap(), -5_000_000_000);
        assert_eq!(decoder.optional_text().unwrap().as_deref(), Some("Zürich"));
        assert_eq!(decoder.optional_text().unwrap(), None);
        assert_eq!(decoder.ids().unwrap(), vec![10, 12, 11, 9_000_000_000]);
        assert_eq!(decoder.next_unsigned().unwrap(), None);
    }
}
