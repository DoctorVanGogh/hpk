use std::io::prelude::*;
use std::io;
use std::io::Cursor;
use std::io::SeekFrom;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;

use hpk;

macro_rules! itry {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(err) => return Some(Err(From::from(err))),
        }
    }
}

pub fn walk<P: AsRef<Path>>(file: P) -> io::Result<HpkIter> {
    let file = file.as_ref().to_path_buf();
    let mut f = File::open(&file)?;

    let hdr = hpk::Header::read_from(&mut f)?;
    let mut fragments_data = Cursor::new(vec![0; hdr.fragmented_filesystem_count as usize]);

    f.seek(SeekFrom::Start(hdr.fragmented_filesystem_offset))?;
    f.read_exact(fragments_data.get_mut().as_mut_slice())?;

    let mut fragments = Vec::with_capacity(hdr.filesystem_entries());
    for _ in 0..hdr.filesystem_entries() {
        fragments.push(hpk::Fragment::read_nth_from(
            hdr.fragments_per_file as usize,
            &mut fragments_data,
        )?);
    }

    let mut residual_data = Cursor::new(vec![0; (hdr.fragments_residual_count * 8) as usize]);

    f.seek(SeekFrom::Start(hdr.fragments_residual_offset))?;
    f.read_exact(residual_data.get_mut().as_mut_slice())?;

    let residual_count = hdr.fragments_residual_count;
    let residuals = hpk::Fragment::read_nth_from(residual_count as usize, &mut residual_data)?;

    Ok(HpkIter {
        file,
        f,
        header: hdr,
        start: Some(hpk::DirEntry::new_root()),
        fragments,
        residuals,
        stack_list: vec![],
    })
}

pub struct HpkIter {
    file: PathBuf,
    f: File,
    header: hpk::Header,
    start: Option<hpk::DirEntry>,
    pub fragments: Vec<Vec<hpk::Fragment>>,
    pub residuals: Vec<hpk::Fragment>,
    stack_list: Vec<DirList>,
}

struct DirList {
    entries: Vec<hpk::DirEntry>,
}

impl Iterator for HpkIter {
    type Item = io::Result<hpk::DirEntry>;

    fn next(&mut self) -> Option<io::Result<hpk::DirEntry>> {
        if let Some(dent) = self.start.take() {
            if let Some(result) = self.handle_entry(dent) {
                return Some(result);
            }
        }
        while !self.stack_list.is_empty() {
            match self.stack_list.last_mut().expect("bug?").next() {
                None => self.pop(),
                Some(Err(err)) => return Some(Err(err)),
                Some(Ok(dent)) => {
                    if let Some(result) = self.handle_entry(dent) {
                        return Some(result);
                    }
                }
            }
        }
        None
    }
}

impl HpkIter {

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn header(&self) -> &hpk::Header {
        &self.header
    }

    pub fn read_file<F>(&self, entry: &hpk::DirEntry, op: F)
    where
        F: FnOnce(hpk::FragmentedReader<&File>) -> (),
    {
        if !entry.is_dir() {
            let fragments = &self.fragments[entry.index()];
            let fragments: Vec<_> = fragments.iter().cloned().collect();
            let r = hpk::FragmentedReader::new(&self.f, fragments);
            op(r);
        }
    }

    fn handle_entry(&mut self, dent: hpk::DirEntry) -> Option<io::Result<hpk::DirEntry>> {
        if dent.is_dir() {
            itry!(self.push(&dent));
        }
        Some(Ok(dent))
    }

    fn push(&mut self, dent: &hpk::DirEntry) -> io::Result<()> {
        let fragment = &self.fragments[dent.index()][0];
        let mut dir_entries = Cursor::new(vec![0; fragment.length as usize]);

        self.f.seek(SeekFrom::Start(fragment.offset))?;
        self.f.read_exact(&mut dir_entries.get_mut().as_mut_slice())?;

        let mut list = vec![];
        while dir_entries.position() < fragment.length {
            let entry = hpk::DirEntry::read_from(dent.path(), dent.depth + 1, &mut dir_entries)?;
            list.push(entry);
        }
        self.stack_list.push(DirList { entries: list });
        Ok(())
    }

    fn pop(&mut self) {
        self.stack_list.pop().expect("cannot pop from empty stack");
    }
}

impl Iterator for DirList {
    type Item = io::Result<hpk::DirEntry>;

    fn next(&mut self) -> Option<io::Result<hpk::DirEntry>> {
        if !self.entries.is_empty() {
            Some(Ok(self.entries.remove(0)))
        } else {
            None
        }
    }
}