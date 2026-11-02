//! The fabrication and assembly drawings as PDF, laid out from the core's
//! `fabrication::DrawingData` on the small canvas in [`crate::pdf`].
//!
//! CONTRACT STUB: filled by the drawings task.
use crate::Result;
use pcbcraft_core::fabrication::DrawingData;

/// The fabrication drawing: outline with dimensions, drill table, stackup
/// table and notes.
pub fn fabrication_pdf(_data: &DrawingData) -> Result<Vec<u8>> {
    Ok(Vec::new())
}

/// The assembly drawing: one page per populated side with designators and
/// polarity marks.
pub fn assembly_pdf(_data: &DrawingData) -> Result<Vec<u8>> {
    Ok(Vec::new())
}
