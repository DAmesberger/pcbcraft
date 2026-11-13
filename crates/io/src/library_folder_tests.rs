//! Folder reader and writer: manifest refusal, immutable revisions, the
//! asset store, labels and git-friendly output. Fixtures are original.
use super::*;
use sparksmith_core::fields::{FieldDefinition, QuantityKind, ValueType};
use sparksmith_core::ids::{DefinitionKind, QuantityKindRef};
use sparksmith_core::library::{Definition, DefinitionMeta};

/// A link-free field record (it names no other lineage).
pub(crate) fn field(lineage: u128, revision: u32, name: &str) -> RevisionRecord {
    RevisionRecord {
        lineage: SourceLineageId(lineage),
        revision: Revision(revision),
        kind: DefinitionKind::Field,
        meta: DefinitionMeta {
            name: name.into(),
            description: format!("Original fixture field {name}"),
            provenance: "Invented for sparksmith-io tests".into(),
        },
        definition: Definition::Field(FieldDefinition::default()),
        links: Vec::new(),
        assets: Vec::new(),
    }
}

pub(crate) fn asset(bytes: &[u8]) -> PackedAsset {
    PackedAsset {
        hash: asset_hash(bytes),
        media_type: "application/pdf".into(),
        data: AssetBytes::new(bytes.to_vec()),
    }
}

pub(crate) fn bundle(
    folder: &Folder,
    records: Vec<RevisionRecord>,
    assets: Vec<PackedAsset>,
) -> Bundle {
    let root = records
        .last()
        .map(|r| SourceRevision {
            lineage: r.lineage,
            revision: r.revision,
        })
        .unwrap_or(SourceRevision {
            lineage: SourceLineageId(0),
            revision: Revision(1),
        });
    Bundle {
        library: folder.manifest.library,
        name: folder.manifest.name.clone(),
        root,
        records,
        assets,
    }
}

/// A Part revision whose datasheet reference names an attached copy of
/// `bytes`, with its whole closure: the synthetic `ExampleCo` catalog
/// exported to `folder` under lineage ids from `first_lineage` up. Only a
/// Part's references name assets, so this is the smallest valid bundle
/// that carries one.
pub(crate) fn part_bundle(
    folder: &Folder,
    bytes: &[u8],
    first_lineage: u128,
) -> Result<(Bundle, PackedAsset)> {
    let mut library = sparksmith_core::catalog::synthetic::project_with_parts(1)?.library;
    let packed = asset(bytes);
    let part = library
        .parts
        .first_mut()
        .ok_or_else(|| Error::Invalid("the synthetic catalog has no Part".into()))?;
    let reference = part
        .definition
        .references
        .first_mut()
        .ok_or_else(|| Error::Invalid("the synthetic Part has no reference".into()))?;
    reference.asset = Some(packed.hash.to_string());
    let root = sparksmith_core::ids::DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    };
    library.assets.push(packed.clone());
    let fresh: Vec<SourceLineageId> = (0..64)
        .map(|i| SourceLineageId(first_lineage + i))
        .collect();
    let plan = sparksmith_core::library::import::export(
        &library,
        root,
        folder.manifest.library,
        &folder.manifest.name,
        &fresh,
    )?;
    Ok((plan.bundle, packed))
}

fn sorted(mut records: Vec<RevisionRecord>) -> Vec<RevisionRecord> {
    records.sort_by_key(|r| (r.lineage, r.revision));
    records
}

fn manifest_text(format: u32, features: &[&str]) -> String {
    let features: Vec<String> = features.iter().map(|f| format!("\"{f}\"")).collect();
    format!(
        "{{\"format\":{format},\"library\":\"000000000000000000000000000000aa\",\"name\":\"Bench\",\"required_features\":[{}],\"default_licence\":null}}",
        features.join(",")
    )
}

#[test]
fn create_then_open_gives_the_same_folder() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let created = create_folder(dir.path(), "Team passives")?;
    let opened = open_folder(dir.path())?;
    assert_eq!(opened, created);
    assert_eq!(inspect_folder(dir.path()), Ok(created.clone()));
    let bytes = std::fs::read(dir.path().join(MANIFEST_FILE))?;
    assert_eq!(bytes, pretty(&created.manifest)?);
    assert_eq!(bytes.last(), Some(&b'\n'));
    let again = create_folder(dir.path(), "Team passives");
    assert_eq!(again.map(|_| ()).map_err(|e| e.code()), Err("conflict"));
    assert!(create_folder(&dir.path().join("x"), " ").is_err());
    // Two folders never share an identity.
    let other = tempfile::tempdir()?;
    assert_ne!(
        create_folder(other.path(), "Team passives")?
            .manifest
            .library,
        created.manifest.library
    );
    Ok(())
}

#[test]
fn corrupt_or_unknown_feature_manifests_are_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    assert_eq!(inspect_folder(dir.path()), Err(Disabled::PathMissing));
    assert!(open_folder(dir.path()).is_err());

    let path = dir.path().join(MANIFEST_FILE);
    std::fs::write(&path, "{\"format\":1,\"library\":")?;
    assert_eq!(
        open_folder(dir.path()).map(|_| ()).map_err(|e| e.code()),
        Err("malformed_file")
    );
    assert!(matches!(
        inspect_folder(dir.path()),
        Err(Disabled::Malformed(_))
    ));

    std::fs::write(
        &path,
        manifest_text(1, &[]).replace("\"name\"", "\"surprise\":1,\"name\""),
    )?;
    assert_eq!(
        open_folder(dir.path()).map(|_| ()).map_err(|e| e.code()),
        Err("malformed_file")
    );

    std::fs::write(&path, manifest_text(1, &["sharded-lineages"]))?;
    assert_eq!(
        open_folder(dir.path()).map(|_| ()).map_err(|e| e.code()),
        Err("unsupported")
    );
    assert_eq!(
        inspect_folder(dir.path()),
        Err(Disabled::UnknownFeatures(vec!["sharded-lineages".into()]))
    );

    std::fs::write(&path, manifest_text(2, &[]))?;
    assert_eq!(
        open_folder(dir.path()).map(|_| ()).map_err(|e| e.code()),
        Err("unsupported")
    );
    assert_eq!(
        inspect_folder(dir.path()),
        Err(Disabled::UnsupportedFormat(2))
    );

    // A newer format that adds a field is unsupported, not unreadable: the
    // version is checked before the strict parse.
    std::fs::write(
        &path,
        manifest_text(2, &[]).replace("\"name\"", "\"shards\":16,\"name\""),
    )?;
    assert_eq!(
        open_folder(dir.path()).map(|_| ()).map_err(|e| e.code()),
        Err("unsupported")
    );
    assert_eq!(
        inspect_folder(dir.path()),
        Err(Disabled::UnsupportedFormat(2))
    );
    std::fs::write(
        &path,
        manifest_text(1, &["sharded-lineages"]).replace("\"name\"", "\"shards\":16,\"name\""),
    )?;
    assert_eq!(
        inspect_folder(dir.path()),
        Err(Disabled::UnknownFeatures(vec!["sharded-lineages".into()]))
    );

    std::fs::write(&path, manifest_text(1, &[]))?;
    let folder = open_folder(dir.path())?;
    assert_eq!(folder.manifest.library, LibraryId(0xaa));
    Ok(())
}

#[test]
fn bundles_write_one_pretty_file_per_revision_and_read_back() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    let (first, pdf) = part_bundle(&folder, b"%PDF-1.7 original fixture", 0x100)?;
    let report = write_bundle(&folder, &first)?;
    let mut expected: Vec<String> = first
        .records
        .iter()
        .map(|r| record_path(r.lineage, r.revision))
        .collect();
    expected.sort();
    assert_eq!(report.written, expected);
    assert_eq!(report.assets, vec![pdf.hash]);
    for record in &first.records {
        let bytes = std::fs::read(
            dir.path()
                .join(record_path(record.lineage, record.revision)),
        )?;
        assert_eq!(
            bytes,
            pretty(record)?,
            "pretty JSON in the types' fixed key order"
        );
    }
    assert_eq!(
        std::fs::read(dir.path().join(asset_path(pdf.hash)))?,
        pdf.data.to_vec()
    );

    let stored = read_records(&folder)?;
    assert_eq!(stored.len(), first.records.len());
    let originals = sorted(first.records.clone());
    for (stored, record) in stored.iter().zip(&originals) {
        assert_eq!(&stored.record, record);
        assert_eq!(stored.content_hash, content_hash(record)?);
    }

    let read = read_bundle(&folder, first.root)?;
    assert_eq!(sorted(read.records.clone()), originals);
    assert_eq!(read.assets, vec![pdf]);
    assert_eq!(read.library, folder.manifest.library);
    assert_eq!(read.root, first.root);

    // Writing the same bundle again changes nothing.
    let again = write_bundle(&folder, &first)?;
    assert!(again.written.is_empty());
    assert_eq!(again.unchanged.len(), first.records.len());
    assert!(again.assets.is_empty());

    // r+1 is a new file beside r1.
    write_bundle(
        &folder,
        &bundle(&folder, vec![field(1, 1, "Tolerance")], vec![]),
    )?;
    let next = bundle(&folder, vec![field(1, 2, "Tolerance, revised")], vec![]);
    assert_eq!(write_bundle(&folder, &next)?.written.len(), 1);
    assert_eq!(read_records(&folder)?.len(), first.records.len() + 2);
    Ok(())
}

#[test]
fn an_existing_revision_is_never_overwritten() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    write_bundle(
        &folder,
        &bundle(&folder, vec![field(1, 1, "Tolerance")], vec![]),
    )?;
    let path = dir
        .path()
        .join(record_path(SourceLineageId(1), Revision(1)));
    let before = std::fs::read(&path)?;

    // New records and a new asset beside one record edited in place.
    let (parts, late) = part_bundle(&folder, b"would be an orphan", 0x300)?;
    let mut records = parts.records.clone();
    records.push(field(1, 1, "Tolerance, edited in place"));
    let clash = Bundle {
        records,
        ..parts.clone()
    };
    let refused = write_bundle(&folder, &clash);
    assert_eq!(refused.map(|_| ()).map_err(|e| e.code()), Err("conflict"));
    assert_eq!(std::fs::read(&path)?, before);
    assert!(
        !dir.path().join(asset_path(late.hash)).exists(),
        "nothing is written"
    );
    for record in &parts.records {
        assert!(
            !dir.path()
                .join(record_path(record.lineage, record.revision))
                .exists(),
            "nothing is written"
        );
    }

    let foreign = Bundle {
        library: LibraryId(folder.manifest.library.0 ^ 1),
        ..bundle(&folder, vec![field(4, 1, "Elsewhere")], vec![])
    };
    assert!(write_bundle(&folder, &foreign).is_err());
    Ok(())
}

#[test]
fn a_modified_revision_is_detected_by_hash() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    let original = field(1, 1, "Tolerance");
    let recorded = content_hash(&original)?;
    write_bundle(&folder, &bundle(&folder, vec![original.clone()], vec![]))?;

    // Someone edits r0001 in place (a bad merge or a hand edit).
    let path = dir
        .path()
        .join(record_path(SourceLineageId(1), Revision(1)));
    let text = std::fs::read_to_string(&path)?;
    std::fs::write(&path, text.replace("Tolerance", "Tolerance (edited)"))?;

    let stored = read_records(&folder)?;
    assert_eq!(stored.len(), 1);
    assert_ne!(
        stored[0].content_hash, recorded,
        "the edit changes the content hash"
    );
    // Republishing the original refuses rather than hiding the edit.
    assert_eq!(
        write_bundle(&folder, &bundle(&folder, vec![original], vec![]))
            .map(|_| ())
            .map_err(|e| e.code()),
        Err("conflict")
    );
    // Whitespace-only reformatting keeps the meaning and the hash.
    let reformatted = serde_json::to_vec(&stored[0].record)?;
    std::fs::write(&path, reformatted)?;
    assert_eq!(
        read_records(&folder)?[0].content_hash,
        stored[0].content_hash
    );
    Ok(())
}

#[test]
fn records_must_match_their_file_names() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    write_bundle(
        &folder,
        &bundle(&folder, vec![field(1, 1, "Tolerance")], vec![]),
    )?;
    let lineage_dir = dir.path().join("lineages/00000000000000000000000000000001");
    // Dot files (git, Finder) are ignored.
    std::fs::write(lineage_dir.join(".DS_Store"), b"x")?;
    std::fs::create_dir_all(dir.path().join("lineages/.git-keep"))?;
    assert_eq!(read_records(&folder)?.len(), 1);

    // A record copied under another revision's name.
    let r1 = std::fs::read(lineage_dir.join("r0001.json"))?;
    std::fs::write(lineage_dir.join("r0002.json"), &r1)?;
    assert_eq!(
        read_records(&folder).map(|_| ()).map_err(|e| e.code()),
        Err("malformed_file")
    );
    std::fs::remove_file(lineage_dir.join("r0002.json"))?;

    for stray in ["r1.json", "r00001.json", "r0000.json", "notes.txt"] {
        std::fs::write(lineage_dir.join(stray), &r1)?;
        assert!(read_records(&folder).is_err(), "{stray} is refused");
        std::fs::remove_file(lineage_dir.join(stray))?;
    }
    std::fs::create_dir_all(dir.path().join("lineages/NOT-AN-ID"))?;
    assert!(read_records(&folder).is_err());
    Ok(())
}

#[test]
fn record_names_are_canonical() {
    assert_eq!(parse_record_name("r0001.json"), Some(Revision(1)));
    assert_eq!(parse_record_name("r12345.json"), Some(Revision(12345)));
    for bad in [
        "r1.json",
        "r00001.json",
        "r0000.json",
        "R0001.json",
        "r0001.JSON",
        "r+001.json",
    ] {
        assert_eq!(parse_record_name(bad), None, "{bad}");
    }
    assert_eq!(
        record_path(SourceLineageId(1), Revision(12345))
            .rsplit('/')
            .next(),
        Some("r12345.json")
    );
}

#[test]
fn a_damaged_asset_is_refused_and_repaired_by_rewriting() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    let (first, pdf) = part_bundle(&folder, b"%PDF-1.7 another original fixture", 0x100)?;
    write_bundle(&folder, &first)?;
    std::fs::write(dir.path().join(asset_path(pdf.hash)), b"tampered")?;
    assert!(read_bundle(&folder, first.root).is_err());
    // Content addressing makes restoring the right bytes safe.
    assert_eq!(write_bundle(&folder, &first)?.assets, vec![pdf.hash]);
    assert_eq!(read_bundle(&folder, first.root)?.assets, vec![pdf.clone()]);

    // Another media type for the same bytes is a conflict.
    let mut renamed = first.clone();
    renamed.assets[0].media_type = "image/png".into();
    assert!(write_bundle(&folder, &renamed).is_err());

    // A record naming an asset nobody holds is refused.
    let other = tempfile::tempdir()?;
    let empty = create_folder(other.path(), "Bench")?;
    let (mut orphan, _) = part_bundle(&empty, b"absent", 0x500)?;
    orphan.assets.clear();
    assert!(write_bundle(&empty, &orphan).is_err());
    assert!(read_records(&empty)?.is_empty(), "nothing is written");
    Ok(())
}

#[test]
fn labels_round_trip_and_refuse_other_formats() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    let empty = read_labels(&folder)?;
    assert_eq!(empty.format, CATALOG_FORMAT);
    assert!(empty.lineages.is_empty());

    let mut labels = empty;
    labels.lineages.insert(
        SourceLineageId(9),
        Labels {
            shelf: Some("Passives/Resistors".into()),
            tags: vec!["preferred".into(), "0603".into()],
        },
    );
    labels
        .lineages
        .insert(SourceLineageId(3), Labels::default());
    write_labels(&folder, &labels)?;
    assert_eq!(read_labels(&folder)?, labels);
    let text = std::fs::read_to_string(dir.path().join(CATALOG_FILE))?;
    assert!(text.ends_with('\n'));
    let three = text.find("00000000000000000000000000000003");
    let nine = text.find("00000000000000000000000000000009");
    assert!(three < nine, "lineages are written in id order");

    labels.lineages.insert(
        SourceLineageId(4),
        Labels {
            shelf: Some("\u{7}".into()),
            tags: vec![],
        },
    );
    assert!(write_labels(&folder, &labels).is_err());
    std::fs::write(
        dir.path().join(CATALOG_FILE),
        "{\"format\":2,\"lineages\":{}}",
    )?;
    assert_eq!(
        read_labels(&folder).map(|_| ()).map_err(|e| e.code()),
        Err("unsupported")
    );
    Ok(())
}

/// Generated link-free records written and read back give equal records
/// and hashes, and equal bytes in a second folder (git-friendly output is a
/// pure function of the records).
#[test]
fn write_read_round_trip_over_generated_records() -> Result<()> {
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let fa = create_folder(a.path(), "Round trip")?;
    let fb = create_folder(b.path(), "Round trip")?;
    let mut seed = 0x2545_f491_u64;
    let mut records = Vec::new();
    for i in 0..40u32 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let lineage = u128::from(seed >> 8) << 32 | u128::from(i);
        let mut record = field(lineage, 1 + (i % 3), &format!("Field {i} · µ {seed:x}"));
        if let Definition::Field(body) = &mut record.definition {
            body.next_local = 1 + (i % 5);
            if i % 2 == 0 {
                body.value_type = ValueType::Integer {
                    min: -(i64::from(i)),
                    max: i64::from(i) * 1000,
                };
            }
        }
        records.push(record);
    }
    write_bundle(&fa, &bundle(&fa, records.clone(), vec![]))?;
    write_bundle(&fb, &bundle(&fb, records.clone(), vec![]))?;
    let stored = read_records(&fa)?;
    assert_eq!(stored.len(), records.len());
    for record in &records {
        let path = record_path(record.lineage, record.revision);
        let found = stored.iter().find(|s| s.path == path);
        assert_eq!(found.map(|s| &s.record), Some(record));
        assert_eq!(found.map(|s| s.content_hash), Some(content_hash(record)?));
        assert_eq!(
            std::fs::read(a.path().join(&path))?,
            std::fs::read(b.path().join(&path))?
        );
    }
    Ok(())
}

/// A field whose value type names a quantity kind: the closure follows the
/// link and puts the dependency first.
#[test]
fn read_bundle_follows_links_dependencies_first() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    let kind = RevisionRecord {
        lineage: SourceLineageId(10),
        revision: Revision(2),
        kind: DefinitionKind::QuantityKind,
        meta: DefinitionMeta::default(),
        definition: Definition::QuantityKind(QuantityKind {
            units: Default::default(),
            default_unit: String::new(),
        }),
        links: Vec::new(),
        assets: Vec::new(),
    };
    let older = RevisionRecord {
        revision: Revision(1),
        ..kind.clone()
    };
    let mut quantity = field(11, 1, "Resistance");
    quantity.definition = Definition::Field(FieldDefinition {
        value_type: ValueType::Quantity {
            kind: QuantityKindRef::new(DefinitionId(1), Revision(2)),
            min: None,
            max: None,
        },
        ..FieldDefinition::default()
    });
    quantity.links = vec![sparksmith_core::library::bundle::Link {
        local: DefinitionId(1),
        lineage: SourceLineageId(10),
    }];
    write_bundle(
        &folder,
        &bundle(&folder, vec![older, kind.clone(), quantity.clone()], vec![]),
    )?;
    let read = read_bundle(
        &folder,
        SourceRevision {
            lineage: SourceLineageId(11),
            revision: Revision(1),
        },
    )?;
    assert_eq!(read.records, vec![kind, quantity]);
    Ok(())
}

/// `read_bundle` reads only the closure: an unrelated record that does not
/// even parse is never opened, and a soft dependency's latest revision is
/// found from its directory listing.
#[test]
fn read_bundle_reads_only_the_closure() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = create_folder(dir.path(), "Bench")?;
    write_bundle(
        &folder,
        &bundle(
            &folder,
            vec![field(1, 1, "Tolerance"), field(2, 1, "Voltage")],
            vec![],
        ),
    )?;
    let unrelated = dir
        .path()
        .join(record_path(SourceLineageId(2), Revision(1)));
    std::fs::write(&unrelated, "{ damaged")?;
    let read = read_bundle(
        &folder,
        SourceRevision {
            lineage: SourceLineageId(1),
            revision: Revision(1),
        },
    )?;
    assert_eq!(read.records, vec![field(1, 1, "Tolerance")]);
    // Reading the damaged one is still refused.
    let damaged = read_bundle(
        &folder,
        SourceRevision {
            lineage: SourceLineageId(2),
            revision: Revision(1),
        },
    );
    assert_eq!(
        damaged.map(|_| ()).map_err(|e| e.code()),
        Err("malformed_file")
    );
    let missing = read_bundle(
        &folder,
        SourceRevision {
            lineage: SourceLineageId(1),
            revision: Revision(9),
        },
    );
    assert_eq!(missing.map(|_| ()).map_err(|e| e.code()), Err("not_found"));
    Ok(())
}
