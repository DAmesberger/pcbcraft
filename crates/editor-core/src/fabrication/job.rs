//! The Gerber job file (`.gbrjob`), from Ucamco's public Gerber Job File
//! format: JSON with the generating software, the project identity, board
//! size, layer count, thickness and finish, the outer-layer design rules,
//! every file with its file function and polarity, and the material
//! stackup top to bottom. Keys are written in sorted order, so the same
//! design gives the same bytes.
use super::artwork;
use super::{FabFile, FabricationOptions, FileGroup};
use crate::diagnostics::Result;
use crate::model::{DielectricKind, GlassStyle, MaskColour, PadFinish, Project, SilkscreenColour};
use crate::rules::CopperObject;
use serde_json::{Value, json};

/// 6 decimals: sums of layer thicknesses print without binary noise.
fn mm(v: f64) -> Value {
    json!((v * 1e6).round() / 1e6)
}

/// The job file's finish name.
pub fn finish(finish: PadFinish) -> &'static str {
    match finish {
        PadFinish::Enig => "ENIG",
        PadFinish::Hasl => "HAL SnPb",
        PadFinish::LeadFreeHasl => "HAL lead-free",
        PadFinish::Osp => "OSP",
        PadFinish::ImmersionSilver => "Immersion Silver",
    }
}

fn mask_colour(c: MaskColour) -> &'static str {
    match c {
        MaskColour::Green => "Green",
        MaskColour::Blue => "Blue",
        MaskColour::Red => "Red",
        MaskColour::Purple => "Purple",
        MaskColour::Yellow => "Yellow",
        MaskColour::Black => "Black",
        MaskColour::White => "White",
        MaskColour::Clear => "Clear",
    }
}

fn legend_colour(c: SilkscreenColour) -> &'static str {
    match c {
        SilkscreenColour::White => "White",
        SilkscreenColour::Black => "Black",
        SilkscreenColour::Yellow => "Yellow",
    }
}

pub fn glass(g: GlassStyle) -> &'static str {
    match g {
        GlassStyle::Style106 => "106",
        GlassStyle::Style1080 => "1080",
        GlassStyle::Style2116 => "2116",
        GlassStyle::Style7628 => "7628",
    }
}

/// The file function and polarity a file declares in its own attributes
/// (`%TF.FileFunction,…*%` or `; #@! TF.FileFunction,…`).
fn declared(file: &FabFile) -> Option<(String, String)> {
    let text = std::str::from_utf8(&file.bytes).ok()?;
    let mut function = None;
    let mut polarity = None;
    for line in text.lines().take(64) {
        let line = line
            .trim_start_matches("; #@! ")
            .trim_start_matches('%')
            .trim_end_matches('%')
            .trim_end_matches('*');
        if let Some(v) = line.strip_prefix("TF.FileFunction,") {
            function = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("TF.FilePolarity,") {
            polarity = Some(v.to_string());
        }
    }
    Some((function?, polarity.unwrap_or_else(|| "Positive".into())))
}

/// The `.gbrjob` describing the Gerber and drill files among `files`.
pub fn file(
    project: &Project,
    options: &FabricationOptions,
    stem: &str,
    files: &[FabFile],
) -> Result<FabFile> {
    let board = project.board().bounds();
    let stackup = &project.stackup;
    let rules = project.rules.effective();
    let c = &rules.copper.clearance;
    let mut header = json!({
        "GenerationSoftware": {
            "Vendor": "Pcbcraft",
            "Application": "Pcbcraft",
            "Version": env!("CARGO_PKG_VERSION"),
        },
    });
    if let (Some(date), Some(h)) = (&options.creation_date, header.as_object_mut()) {
        h.insert("CreationDate".into(), json!(date));
    }
    let files_attributes: Vec<Value> = files
        .iter()
        .filter(|f| matches!(f.group, FileGroup::Gerber | FileGroup::Drill))
        .filter_map(|f| {
            let (function, polarity) = declared(f)?;
            Some(json!({
                "Path": f.name,
                "FileFunction": function,
                "FilePolarity": polarity,
            }))
        })
        .collect();
    let fab = &project.fabrication;
    let mut materials = Vec::new();
    if stackup.silkscreen_top {
        materials.push(json!({"Type": "Legend", "Name": "Top legend", "Color": legend_colour(fab.silkscreen_colour)}));
    }
    if stackup.mask_top {
        materials.push(json!({"Type": "SolderMask", "Name": "Top solder mask", "Color": mask_colour(fab.mask_colour)}));
    }
    let layers = stackup.layers();
    for (i, layer) in layers.iter().enumerate() {
        if let Some(foil) = stackup.copper.get(i) {
            materials.push(json!({
                "Type": "Copper",
                "Name": format!("{} copper", layer.name()),
                "Thickness": mm(foil.thickness_um / 1000.0),
            }));
        }
        if let Some(d) = stackup.dielectrics.get(i) {
            materials.push(json!({
                "Type": "Dielectric",
                "Name": format!("{} {}", match d.kind { DielectricKind::Core => "Core", DielectricKind::Prepreg => "Prepreg" }, i + 1),
                "Material": "FR4",
                "Thickness": mm(d.thickness_mm),
                "Notes": format!("{} glass", glass(d.glass)),
            }));
        }
    }
    if stackup.mask_bottom {
        materials.push(json!({"Type": "SolderMask", "Name": "Bottom solder mask", "Color": mask_colour(fab.mask_colour)}));
    }
    if stackup.silkscreen_bottom {
        materials.push(json!({"Type": "Legend", "Name": "Bottom legend", "Color": legend_colour(fab.silkscreen_colour)}));
    }
    let job = json!({
        "Header": header,
        "GeneralSpecs": {
            "ProjectId": {
                "Name": project.name,
                "GUID": artwork::project_guid(project),
                "Revision": "1",
            },
            "Size": {"X": mm(board.size()[0]), "Y": mm(board.size()[1])},
            "LayerNumber": layers.len(),
            "BoardThickness": mm(stackup.thickness_mm()),
            "Finish": finish(fab.pad_finish),
        },
        "DesignRules": [{
            "Layers": "Outer",
            "PadToPad": mm(c.get(CopperObject::Pad, CopperObject::Pad)),
            "PadToTrack": mm(c.get(CopperObject::Pad, CopperObject::Track)),
            "TrackToTrack": mm(c.get(CopperObject::Track, CopperObject::Track)),
            "TrackToRegion": mm(c.get(CopperObject::Track, CopperObject::Fill)),
            "RegionToRegion": mm(c.get(CopperObject::Fill, CopperObject::Fill)),
            "MinLineWidth": mm(rules.copper.track_width_min),
        }],
        "FilesAttributes": files_attributes,
        "MaterialStackup": materials,
    });
    let mut bytes = serde_json::to_vec_pretty(&job)?;
    bytes.push(b'\n');
    Ok(FabFile {
        name: format!("{stem}.gbrjob"),
        group: FileGroup::Job,
        description: "Gerber job file (stackup, thickness, finish, layer order)".into(),
        bytes,
    })
}
