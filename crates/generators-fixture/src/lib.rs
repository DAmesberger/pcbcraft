//! A test plugin, never shipped: it offers `generate-part` (effect `none`)
//! beside an `edit` command, so it may import the host functions, and
//! during `generate-part` it calls all three. The host answers `call` with
//! the fixed refusal `plugin.pure`; the module checks that it did and
//! makes its part, which draws an arc of 45°, whose ends the SDK computes
//! in integers. `cargo xtask generators check` runs its golden case
//! natively, under wasmi and under the probe: one result hash on all three
//! shows that a call in a pure invoke changes nothing and that an arc off
//! the quarter turns is the same bits everywhere (part-generation B3, ABI
//! slice S8).
#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use sparksmith_generator_sdk::abi::codec;
use sparksmith_generator_sdk::draw;
use sparksmith_generator_sdk::host;
use sparksmith_generator_sdk::v1::{
    self, Capability, Description, Effect, Fill, Footprint, Generation, Graphic, Lead, Model, Need,
    Opening, Package, PackageGeometry, Pad, PadShape, PartOffer, Problem, Request, Scope, Side,
};
use sparksmith_generator_sdk::{Generator, export_generator};

pub struct Fixture;

fn problem(code: &str, message: &str) -> Vec<Problem> {
    vec![Problem {
        code: code.into(),
        field: None,
        message: message.into(),
    }]
}

impl Generator for Fixture {
    fn describe() -> Description {
        Description {
            abi: v1::ABI,
            id: "fixture".into(),
            version: 1,
            title: "Host-call fixture".into(),
            licence: "MIT OR Apache-2.0".into(),
            deterministic: true,
            capabilities: vec![
                Capability {
                    name: v1::GENERATE_PART.into(),
                    effect: Effect::None,
                    title: Some("A one-pad part with a 45° arc".into()),
                    params: Vec::new(),
                    rules: Vec::new(),
                    presets: Vec::new(),
                    part: Some(PartOffer {
                        families: Vec::new(),
                        scopes: vec![Scope::Package],
                        grammar: Vec::new(),
                        instance_fields: Vec::new(),
                    }),
                },
                Capability {
                    name: "touch".into(),
                    effect: Effect::Edit,
                    title: Some("An edit command (never served in this version)".into()),
                    params: Vec::new(),
                    rules: Vec::new(),
                    presets: Vec::new(),
                    part: None,
                },
            ],
            needs: vec![Need {
                module: "layout".into(),
                major: 1,
                minor: 0,
                class: Effect::Edit,
            }],
        }
    }

    fn generate(_request: &Request) -> Result<Generation, Vec<Problem>> {
        // A pure invoke: the host refuses the call, drops the log line and
        // never cancels.
        host::log(1, b"generate-part asks the host for something");
        if host::cancelled() {
            return Err(problem("fixture.cancelled", "Cancelled in a pure invoke."));
        }
        let mut expected = vec![v1::STATUS_ERR];
        expected.extend(codec::encode(&v1::pure_refusal()).unwrap_or_default());
        if host::call(br#"{"entry":"layout.add_via","params":{}}"#) != expected {
            return Err(problem(
                "fixture.call",
                "The host answered a call in a pure invoke with something other than plugin.pure.",
            ));
        }
        let arc = draw::arc([0, 0], 1_000_000, 0, 45_000)
            .ok_or_else(|| problem("fixture.arc", "The arc overflowed."))?;
        Ok(Generation {
            component: None,
            symbol: None,
            package: Package {
                terminals: vec!["1".into()],
                geometry: PackageGeometry {
                    envelope_nm: [2_000_000, 2_000_000, 500_000],
                    seating_nm: 0,
                    leads: vec![Lead {
                        key: "1".into(),
                        contact: [0, 0, 0],
                    }],
                    pin1: "1".into(),
                },
            },
            footprint: Footprint {
                pads: vec![Pad {
                    key: "1".into(),
                    terminal: "1".into(),
                    shape: PadShape::Rect {
                        size: [500_000, 500_000],
                    },
                    at: [0, 0],
                    rot_mdeg: 0,
                    side: Side::Top,
                    hole: None,
                    mask: Opening::Default,
                    paste: Opening::Default,
                }],
                courtyard: vec![
                    [-1_250_000, -1_250_000],
                    [1_250_000, -1_250_000],
                    [1_250_000, 1_250_000],
                    [-1_250_000, 1_250_000],
                ],
                silk: Vec::new(),
                assembly: vec![Graphic {
                    key: "arc".into(),
                    shape: arc,
                    stroke_nm: 100_000,
                    fill: Fill::None,
                }],
            },
            binding: Vec::new(),
            model: Some(Model::PartFile {
                asset: "ab".repeat(32),
                values: Default::default(),
            }),
            fields: Vec::new(),
            notes: Vec::new(),
        })
    }
}

export_generator!(Fixture);

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_generator_sdk::golden;

    #[test]
    fn golden_hashes_match_natively() -> Result<(), String> {
        assert_eq!(golden::check::<Fixture>(include_str!("../golden.json"))?, 1);
        Ok(())
    }

    #[test]
    fn the_arc_ends_are_integers_at_45_degrees() -> Result<(), Vec<Problem>> {
        let request = Request {
            abi: v1::ABI,
            scope: Scope::Package,
            params: Default::default(),
        };
        let g = Fixture::generate(&request)?;
        assert_eq!(
            g.footprint.assembly.first().map(|a| a.shape.clone()),
            Some(v1::Primitive::Arc {
                center: [0, 0],
                start: [1_000_000, 0],
                end: [707_107, 707_107],
                clockwise: false,
            })
        );
        Ok(())
    }
}
