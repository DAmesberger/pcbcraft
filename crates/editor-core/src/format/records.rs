//! The declared field order of every stored record and the sort key of
//! every collection, as data the layout writers use
//! (docs/file-format.md, "Field order" and "Collections").
//!
//! A record's keys are written in this order, identity first: the id (or
//! the key that identifies the record), the record type in a mixed file,
//! then references, names, sizes, geometry and flags. Hashes do not use
//! this order; they use the canonical sorted encoding. The same order is
//! the column order of a table reply. A field the declaration does not
//! list cannot be written, so a new stored field is a change here too.

/// How numbers in a file are checked. Until the units migration (U2, U3)
/// finishes, some stored fields are still `f64` millimetres; their files
/// allow floats, and every other file refuses them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Numbers {
    IntegersOnly,
    /// Floats are still allowed: the file holds a field on the
    /// `f64-lengths` allowlist.
    FloatsAllowed,
}

/// The keys of one record type, in the order they are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordOrder {
    /// The value of the collection's type field, in a mixed collection.
    pub kind: Option<&'static str>,
    pub fields: &'static [&'static str],
}

/// A layout A file: a header line, then one record per line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Collection {
    /// The format name is `sparksmith/<name>`.
    pub name: &'static str,
    /// The path in the tree; `{sheet}` stands for the header's sheet id.
    pub path: &'static str,
    /// Header members after `format` and `version`, in this order.
    pub header: &'static [&'static str],
    /// The field naming each record's type, in a mixed collection.
    pub type_field: Option<&'static str>,
    /// Records are sorted by these fields, in order; each is an integer
    /// or a string, and no two records share the whole key.
    pub sort: &'static [&'static str],
    pub records: &'static [RecordOrder],
    pub numbers: Numbers,
}

impl Collection {
    /// `sparksmith/<name>`, the header's `format`.
    pub fn format(&self) -> String {
        format!("sparksmith/{}", self.name)
    }

    /// The declaration for a record of type `kind` (`None` in a collection
    /// of one type).
    pub fn record(&self, kind: Option<&str>) -> Option<&'static RecordOrder> {
        self.records.iter().find(|r| r.kind == kind)
    }
}

/// A layout B file: one object, members one per line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentLayout {
    /// The format name is `sparksmith/<name>`.
    pub name: &'static str,
    pub path: &'static str,
    /// Members after `format` and `version`, in this order.
    pub fields: &'static [&'static str],
    /// Array members written one element per line.
    pub tables: &'static [&'static str],
    pub numbers: Numbers,
}

impl DocumentLayout {
    pub fn format(&self) -> String {
        format!("sparksmith/{}", self.name)
    }
}

macro_rules! records {
    ($fields:expr) => {
        &[RecordOrder {
            kind: None,
            fields: $fields,
        }]
    };
}

pub const INSTANCES: Collection = Collection {
    name: "instances",
    path: "circuit/instances.jsonl",
    header: &[],
    type_field: None,
    sort: &["id"],
    records: records!(&[
        "id",
        "component",
        "designator",
        "representation",
        "physical",
        "part",
        "properties",
        "unused_units",
        "no_connect",
        "functions",
        "pin_config",
        "marking",
        "text_placements",
    ]),
    numbers: Numbers::FloatsAllowed,
};

pub const NETS: Collection = Collection {
    name: "nets",
    path: "circuit/nets.jsonl",
    header: &[],
    type_field: None,
    sort: &["id"],
    records: records!(&["id", "name", "terminals"]),
    numbers: Numbers::IntegersOnly,
};

/// One file per sheet; the sheet's id is in the header and the records do
/// not repeat it.
pub const SHEET: Collection = Collection {
    name: "sheet",
    path: "schematic/sheet-{sheet}.jsonl",
    header: &["sheet"],
    type_field: Some("type"),
    sort: &["type", "id"],
    records: &[
        RecordOrder {
            kind: Some("drawing"),
            fields: &["id", "type", "item"],
        },
        RecordOrder {
            kind: Some("junction"),
            fields: &["id", "type", "position", "placed"],
        },
        RecordOrder {
            kind: Some("label"),
            fields: &[
                "id", "type", "name", "kind", "position", "rotation", "manual",
            ],
        },
        RecordOrder {
            kind: Some("occurrence"),
            fields: &[
                "id", "type", "instance", "unit", "position", "rotation", "mirror",
            ],
        },
        RecordOrder {
            kind: Some("sheet_symbol"),
            fields: &["id", "type", "child", "position", "size", "pins"],
        },
        RecordOrder {
            kind: Some("wire"),
            fields: &["id", "type", "from", "to", "corners"],
        },
    ],
    numbers: Numbers::IntegersOnly,
};

/// Sorted by instance: a placement is identified by the instance it
/// places.
pub const PLACEMENTS: Collection = Collection {
    name: "placements",
    path: "board/placements.jsonl",
    header: &[],
    type_field: None,
    sort: &["instance"],
    records: records!(&["instance", "layer", "position", "rotation_deg", "locked"]),
    numbers: Numbers::FloatsAllowed,
};

/// Sorted by net, then id: a net's copper is one block of lines.
pub const TRACKS: Collection = Collection {
    name: "tracks",
    path: "board/tracks.jsonl",
    header: &[],
    type_field: None,
    sort: &["net", "id"],
    records: records!(&["id", "net", "layer", "width", "start", "end", "locked"]),
    numbers: Numbers::FloatsAllowed,
};

/// Sorted by net, then id, as tracks are.
pub const VIAS: Collection = Collection {
    name: "vias",
    path: "board/vias.jsonl",
    header: &[],
    type_field: None,
    sort: &["net", "id"],
    records: records!(&[
        "id", "net", "span", "drill", "diameter", "position", "tented", "locked",
    ]),
    numbers: Numbers::FloatsAllowed,
};

/// Fill definitions only: the computed copper is derived data and is not
/// declared, so a fill that still carries it cannot be written (F2 moves
/// it to a cache).
pub const FILLS: Collection = Collection {
    name: "fills",
    path: "board/fills.jsonl",
    header: &[],
    type_field: None,
    sort: &["id"],
    records: records!(&[
        "id", "net", "layer", "name", "region", "priority", "settings", "outline", "locked",
    ]),
    numbers: Numbers::FloatsAllowed,
};

pub const KEEPOUTS: Collection = Collection {
    name: "keepouts",
    path: "board/keepouts.jsonl",
    header: &[],
    type_field: None,
    sort: &["id"],
    records: records!(&["id", "name", "layers", "forbid", "outline"]),
    numbers: Numbers::FloatsAllowed,
};

pub const DIMENSIONS: Collection = Collection {
    name: "dimensions",
    path: "board/dimensions.jsonl",
    header: &[],
    type_field: None,
    sort: &["id"],
    records: records!(&["id", "kind", "refs", "value", "reference"]),
    numbers: Numbers::FloatsAllowed,
};

/// Sorted by the waiver's key, which identifies it.
pub const WAIVERS: Collection = Collection {
    name: "waivers",
    path: "rules/waivers.jsonl",
    header: &[],
    type_field: None,
    sort: &["key"],
    records: records!(&["key", "reason"]),
    numbers: Numbers::IntegersOnly,
};

/// Every collection declared so far.
pub const COLLECTIONS: &[Collection] = &[
    INSTANCES, NETS, SHEET, PLACEMENTS, TRACKS, VIAS, FILLS, KEEPOUTS, DIMENSIONS, WAIVERS,
];

/// The project manifest; `next` holds the id counters.
pub const PROJECT_MANIFEST: DocumentLayout = DocumentLayout {
    name: "project",
    path: "sparksmith-project.json",
    fields: &["features", "name", "material_table", "next"],
    tables: &[],
    numbers: Numbers::IntegersOnly,
};

pub const PROPERTIES: DocumentLayout = DocumentLayout {
    name: "properties",
    path: "properties.json",
    fields: &["title", "revision", "date", "author", "company"],
    tables: &[],
    numbers: Numbers::IntegersOnly,
};

/// Sheets in page order: order is meaning here, so it is a table inside
/// one document, not a sorted collection.
pub const SHEETS: DocumentLayout = DocumentLayout {
    name: "sheets",
    path: "schematic/sheets.json",
    fields: &["sheets"],
    tables: &["sheets"],
    numbers: Numbers::IntegersOnly,
};

pub const BOARD: DocumentLayout = DocumentLayout {
    name: "board",
    path: "board/board.json",
    fields: &["size", "shape", "stackup", "fabrication"],
    tables: &[],
    numbers: Numbers::FloatsAllowed,
};

/// A library folder's manifest.
pub const LIBRARY_MANIFEST: DocumentLayout = DocumentLayout {
    name: "library",
    path: "sparksmith-library.json",
    fields: &["features", "library", "name", "default_licence"],
    tables: &[],
    numbers: Numbers::IntegersOnly,
};

/// Every document layout declared so far.
pub const DOCUMENTS: &[DocumentLayout] = &[
    PROJECT_MANIFEST,
    PROPERTIES,
    SHEETS,
    BOARD,
    LIBRARY_MANIFEST,
];
