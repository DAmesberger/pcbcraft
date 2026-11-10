//! Part dimensions in Layout and PCB 3D: one dimensioning system, drawn
//! and edited the same way in both views (docs/board-sketch-references.md
//! §15, §19, §20).
//!
//! Every part dimension is a measurement in the board plane: z and the
//! seating are fixed, so each reference has a trace there (a face square to
//! the board is a line, an edge a segment, an origin, pad centre or hole a
//! point, a board edge its curve), and `layout::dimensions::view::shown`
//! measures between two traces. Shared, once:
//!
//! - [`state`]: a pick, a drawn dimension, the value open at a label, the
//!   tool's state, the words a reference reads in, and the one selection
//!   (`select_dimension`, with the hover the Properties card shares).
//! - [`draw`]: extension lines, dimension line, arrows and label, laid out
//!   by Solidcraft's sketch dimension layout and stacked, with hover and
//!   selection colours, through a view's board → screen mapping
//!   ([`draw::Surface`]).
//! - [`edit`]: the value edited in place at the label.
//! - [`tool`]: the Dimension tool's state machine (pick A, pick B, type,
//!   `layout.dimension_add`), clicks on drawn dimensions and Delete.
//!
//! Per view, picking only: [`view_3d`] ray-picks the model; [`view_2d`]
//! picks traces on the Layout canvas, the parts' 3D bodies drawn as their
//! body traces, with a chooser where several faces share one trace.
//!
//! Hover, selection and the camera read the document, the cached meshes
//! and the listed body faces only; nothing here rebuilds geometry or traces
//! a model but a pick.
mod draw;
mod edit;
mod state;
mod tool;
mod view_2d;
mod view_3d;

pub(super) use state::Dimensions;
