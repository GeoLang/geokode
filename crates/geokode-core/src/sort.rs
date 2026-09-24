use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub trait SortItem: Ord + Sized {
    fn write_to(&self, out: &mut impl Write) -> io::Result<()>;
    fn read_from(input: &mut impl Read) -> io::Result<Option<Self>>;
    fn memory_bytes(&self) -> usize;
}

fn read_exact_or_end<const N: usize>(input: &mut impl Read) -> io::Result<Option<[u8; N]>> {
    let mut buf = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let read = input.read(&mut buf[filled..])?;
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sort run ends inside an item",
            ));
        }
        filled += read;
    }
    Ok(Some(buf))
}

impl SortItem for i64 {
    fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(&self.to_le_bytes())
    }

    fn read_from(input: &mut impl Read) -> io::Result<Option<Self>> {
        Ok(read_exact_or_end::<8>(input)?.map(i64::from_le_bytes))
    }

    fn memory_bytes(&self) -> usize {
        8
    }
}

impl SortItem for (String, u32) {
    fn write_to(&self, out: &mut impl Write) -> io::Result<()> {
        let length = u32::try_from(self.0.len()).map_err(io::Error::other)?;
        out.write_all(&length.to_le_bytes())?;
        out.write_all(self.0.as_bytes())?;
        out.write_all(&self.1.to_le_bytes())
    }

    fn read_from(input: &mut impl Read) -> io::Result<Option<Self>> {
        let Some(length) = read_exact_or_end::<4>(input)? else {
            return Ok(None);
        };
        let mut key = vec![0u8; u32::from_le_bytes(length) as usize];
        input.read_exact(&mut key)?;
        let mut id = [0u8; 4];
        input.read_exact(&mut id)?;
        let key = String::from_utf8(key).map_err(io::Error::other)?;
        Ok(Some((key, u32::from_le_bytes(id))))
    }

    fn memory_bytes(&self) -> usize {
        self.0.len() + std::mem::size_of::<Self>()
    }
}

// sorts more items than fit in memory by spilling sorted runs to disk
pub struct ExternalSorter<T: SortItem> {
    directory: PathBuf,
    name: String,
    memory_budget: usize,
    buffer: Vec<T>,
    buffered_bytes: usize,
    runs: Vec<PathBuf>,
}

impl<T: SortItem> ExternalSorter<T> {
    pub fn new(directory: &Path, name: &str, memory_budget: usize) -> Self {
        Self {
            directory: directory.to_path_buf(),
            name: name.to_string(),
            memory_budget,
            buffer: Vec::new(),
            buffered_bytes: 0,
            runs: Vec::new(),
        }
    }

    pub fn push(&mut self, item: T) -> io::Result<()> {
        self.buffered_bytes += item.memory_bytes();
        self.buffer.push(item);
        if self.buffered_bytes >= self.memory_budget {
            self.spill()?;
        }
        Ok(())
    }

    fn spill(&mut self) -> io::Result<()> {
        self.buffer.sort_unstable();
        let path = self
            .directory
            .join(format!("{}.run{}", self.name, self.runs.len()));
        let mut out = BufWriter::new(File::create(&path)?);
        for item in self.buffer.drain(..) {
            item.write_to(&mut out)?;
        }
        out.flush()?;
        self.buffered_bytes = 0;
        self.runs.push(path);
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<SortedItems<T>> {
        if self.runs.is_empty() {
            self.buffer.sort_unstable();
            return Ok(SortedItems::Memory(
                std::mem::take(&mut self.buffer).into_iter(),
            ));
        }
        if !self.buffer.is_empty() {
            self.spill()?;
        }
        let mut readers = Vec::with_capacity(self.runs.len());
        let mut heap = BinaryHeap::new();
        for (run, path) in self.runs.iter().enumerate() {
            let mut reader = BufReader::new(File::open(path)?);
            if let Some(item) = T::read_from(&mut reader)? {
                heap.push(Reverse((item, run)));
            }
            readers.push(reader);
        }
        Ok(SortedItems::Merge {
            readers,
            heap,
            runs: std::mem::take(&mut self.runs),
        })
    }
}

pub enum SortedItems<T: SortItem> {
    Memory(std::vec::IntoIter<T>),
    Merge {
        readers: Vec<BufReader<File>>,
        heap: BinaryHeap<Reverse<(T, usize)>>,
        runs: Vec<PathBuf>,
    },
}

impl<T: SortItem> Iterator for SortedItems<T> {
    type Item = io::Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            SortedItems::Memory(items) => items.next().map(Ok),
            SortedItems::Merge { readers, heap, .. } => {
                let Reverse((item, run)) = heap.pop()?;
                match T::read_from(&mut readers[run]) {
                    Ok(Some(next)) => heap.push(Reverse((next, run))),
                    Ok(None) => {}
                    Err(error) => return Some(Err(error)),
                }
                Some(Ok(item))
            }
        }
    }
}

impl<T: SortItem> Drop for SortedItems<T> {
    fn drop(&mut self) {
        if let SortedItems::Merge { runs, .. } = self {
            for path in runs.iter() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spilled_runs_merge_into_one_sorted_stream() {
        let directory = tempfile::tempdir().unwrap();
        let mut sorter = ExternalSorter::new(directory.path(), "ids", 64);
        let values: Vec<i64> = (0..1000).map(|i| (i * 7919) % 1000 - 500).collect();
        for value in &values {
            sorter.push(*value).unwrap();
        }
        let sorted: Vec<i64> = sorter.finish().unwrap().map(Result::unwrap).collect();
        let mut expected = values;
        expected.sort_unstable();
        assert_eq!(sorted, expected);
    }

    #[test]
    fn keys_sort_by_text_then_id() {
        let directory = tempfile::tempdir().unwrap();
        let mut sorter = ExternalSorter::new(directory.path(), "keys", 40);
        for (key, id) in [("zurich", 2), ("bern", 9), ("zurich", 1), ("basel", 4)] {
            sorter.push((key.to_string(), id)).unwrap();
        }
        let sorted: Vec<(String, u32)> = sorter.finish().unwrap().map(Result::unwrap).collect();
        let keys: Vec<(&str, u32)> = sorted.iter().map(|(k, id)| (k.as_str(), *id)).collect();
        assert_eq!(
            keys,
            vec![("basel", 4), ("bern", 9), ("zurich", 1), ("zurich", 2)]
        );
    }
}
