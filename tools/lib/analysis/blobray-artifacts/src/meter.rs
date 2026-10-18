//! Adapt object::ReadRef so delimiter scans have bounded cooperative checkpoints.
use object::read::ReadRef;
use oer_riscv_model::*;
use std::{cell::RefCell, ops::Range};

pub(crate) struct Meter<'a> {
    control: RefCell<&'a mut dyn RunControl>,
    error: RefCell<Option<Error>>,
}
impl<'a> Meter<'a> {
    pub fn new(control: &'a mut dyn RunControl) -> Self {
        Self {
            control: RefCell::new(control),
            error: RefCell::new(None),
        }
    }
    pub fn checkpoint(&self, units: u64) -> Result<()> {
        if let Some(error) = self.error.borrow().as_ref() {
            return Err(error.clone());
        }
        match self.control.borrow_mut().checkpoint(units) {
            Ok(()) => Ok(()),
            Err(error) => {
                *self.error.borrow_mut() = Some(error.clone());
                Err(error)
            }
        }
    }
    pub fn position(&self) -> RunPosition {
        self.control.borrow().position()
    }
    pub fn set_position(&self, position: RunPosition) {
        self.control.borrow_mut().set_position(position);
    }
    pub fn with_control<T>(&self, f: impl FnOnce(&mut dyn RunControl) -> Result<T>) -> Result<T> {
        let result = f(&mut **self.control.borrow_mut());
        if let Err(error) = &result {
            *self.error.borrow_mut() = Some(error.clone());
        }
        result
    }
    pub fn failed(&self) -> bool {
        self.error.borrow().is_some()
    }
    pub fn copy(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.try_reserve_exact(bytes.len()).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "host allocation refused name buffer",
            )
        })?;
        for chunk in bytes.chunks(WORK_BLOCK) {
            self.checkpoint(1)?;
            output.extend_from_slice(chunk);
        }
        Ok(output)
    }
    pub fn finish<T>(&self, value: Result<T>) -> Result<T> {
        match self.error.borrow_mut().take() {
            Some(error) => Err(error),
            None => value,
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Bytes<'data, 'meter, 'control> {
    pub bytes: &'data [u8],
    pub meter: &'meter Meter<'control>,
}
impl<'data> ReadRef<'data> for Bytes<'data, '_, '_> {
    fn len(self) -> std::result::Result<u64, ()> {
        Ok(self.bytes.len() as u64)
    }
    fn read_bytes_at(self, offset: u64, size: u64) -> std::result::Result<&'data [u8], ()> {
        self.meter.checkpoint(0).map_err(|_| ())?;
        // Borrowing a range is O(1); actual scans/copies and visited records pay work.
        self.bytes.read_bytes_at(offset, size)
    }
    fn read_bytes_at_until(
        self,
        range: Range<u64>,
        delimiter: u8,
    ) -> std::result::Result<&'data [u8], ()> {
        let start = usize::try_from(range.start).map_err(|_| ())?;
        let end = usize::try_from(range.end).map_err(|_| ())?;
        let bytes = self.bytes.get(start..end).ok_or(())?;
        for (i, chunk) in bytes.chunks(WORK_BLOCK).enumerate() {
            self.meter.checkpoint(1).map_err(|_| ())?;
            if let Some(index) = chunk.iter().position(|&b| b == delimiter) {
                return Ok(&bytes[..i * WORK_BLOCK + index]);
            }
        }
        Err(())
    }
}
