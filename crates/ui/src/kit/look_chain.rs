//! Appearance inheritance selector. The caller owns selection and commands.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookStep {
    Face,
    Body,
    Component,
    Material,
}

impl LookStep {}
