use super::*;

impl Page {
    pub fn slot_count(&self) -> Result<u16> {
        let header = self.header()?;
        Ok(((header.lower as usize - PAGE_HEADER_LEN) / SLOT_LEN) as u16)
    }

    pub fn insert_cell(&mut self, payload: &[u8]) -> Result<u16> {
        let slot_no = self.slot_count()?;
        self.insert_cell_at(slot_no, payload)
    }

    pub fn insert_cell_at(&mut self, slot: u16, payload: &[u8]) -> Result<u16> {
        self.place_cell(slot, payload)?;
        self.refresh_checksum()?;
        Ok(slot)
    }

    /// Append `cells` in order and refresh the checksum once, not once per
    /// cell: rewriting a page of N cells checksums the page once instead of
    /// N times. On an error the cells placed so far stay, as they would
    /// with [`Page::insert_cell`], and the checksum still covers them.
    pub fn insert_cells<I, C>(&mut self, cells: I) -> Result<()>
    where
        I: IntoIterator<Item = C>,
        C: AsRef<[u8]>,
    {
        let mut placed = Ok(());
        for cell in cells {
            let slot = self.slot_count()?;
            if let Err(err) = self.place_cell(slot, cell.as_ref()) {
                placed = Err(err);
                break;
            }
        }
        self.refresh_checksum()?;
        placed
    }

    /// [`Page::insert_cell_at`] without refreshing the checksum.
    fn place_cell(&mut self, slot: u16, payload: &[u8]) -> Result<()> {
        let mut header = self.header()?;
        let slot_count = self.slot_count()?;
        if slot > slot_count {
            return Err(Error::CorruptPage("slot out of bounds"));
        }
        let new_lower = header.lower as usize + SLOT_LEN;
        let new_upper = (header.upper as usize)
            .checked_sub(payload.len())
            .ok_or_else(|| Error::PageFull)?;

        if new_lower > new_upper {
            return Err(Error::PageFull);
        }
        if payload.len() > u16::MAX as usize {
            return Err(Error::CorruptPage("cell too large"));
        }

        let slot_offset = PAGE_HEADER_LEN + slot as usize * SLOT_LEN;
        let old_lower = header.lower as usize;
        let directory_bytes = old_lower - PAGE_HEADER_LEN;
        let tail = directory_bytes.saturating_sub(slot as usize * SLOT_LEN);
        if tail > 0 {
            let src = slot_offset..old_lower;
            self.bytes.copy_within(src, slot_offset + SLOT_LEN);
        }

        write_bytes(&mut self.bytes, new_upper, payload)?;
        write_u16(&mut self.bytes, slot_offset, new_upper as u16)?;
        write_u16(&mut self.bytes, slot_offset + 2, payload.len() as u16)?;

        header.lower = new_lower as u16;
        header.upper = new_upper as u16;
        self.write_header(&header)
    }

    pub fn delete_cell(&mut self, slot: u16) -> Result<()> {
        let mut header = self.header()?;
        let slot_count = self.slot_count()?;
        if slot >= slot_count {
            return Err(Error::CorruptPage("slot out of bounds"));
        }

        let slot_offset = PAGE_HEADER_LEN + slot as usize * SLOT_LEN;
        let old_lower = header.lower as usize;
        let next = slot_offset + SLOT_LEN;
        if next < old_lower {
            self.bytes.copy_within(next..old_lower, slot_offset);
        }
        header.lower = header.lower.saturating_sub(SLOT_LEN as u16);
        self.write_header(&header)?;
        self.refresh_checksum()
    }

    pub fn special_bytes(&self) -> Result<&[u8]> {
        let header = self.header()?;
        Ok(&self.bytes[header.special as usize..])
    }

    pub fn special_bytes_mut(&mut self) -> Result<&mut [u8]> {
        let header = self.header()?;
        let special = header.special as usize;
        Ok(&mut self.bytes[special..])
    }

    pub fn cell(&self, slot: u16) -> Result<&[u8]> {
        let slot_count = self.slot_count()?;
        if slot >= slot_count {
            return Err(Error::CorruptPage("slot out of bounds"));
        }
        let slot_offset = PAGE_HEADER_LEN + slot as usize * SLOT_LEN;
        let cell_offset = read_u16(&self.bytes, slot_offset)? as usize;
        let cell_len = read_u16(&self.bytes, slot_offset + 2)? as usize;
        let end = cell_offset
            .checked_add(cell_len)
            .ok_or_else(|| Error::CorruptPage("cell overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::CorruptPage("cell extends past page"));
        }
        Ok(&self.bytes[cell_offset..end])
    }

    pub fn overwrite_cell(&mut self, slot: u16, payload: &[u8]) -> Result<()> {
        let slot_count = self.slot_count()?;
        if slot >= slot_count {
            return Err(Error::CorruptPage("slot out of bounds"));
        }

        let slot_offset = PAGE_HEADER_LEN + slot as usize * SLOT_LEN;
        let cell_offset = read_u16(&self.bytes, slot_offset)? as usize;
        let cell_len = read_u16(&self.bytes, slot_offset + 2)? as usize;
        if payload.len() != cell_len {
            return Err(Error::CorruptPage("overwrite cell length mismatch"));
        }

        let end = cell_offset
            .checked_add(cell_len)
            .ok_or_else(|| Error::CorruptPage("cell overflow"))?;
        if end > self.bytes.len() {
            return Err(Error::CorruptPage("cell extends past page"));
        }

        write_bytes(&mut self.bytes, cell_offset, payload)?;
        self.refresh_checksum()
    }
}
