//! S3 local library folders in the core: the reference-rewrite visitor,
//! closures, bundle validation, import/export round trips, place-from,
//! promote, duplicate, folder catalogs and update-available findings. The
//! folder itself is simulated by the records a promote plan carries; io
//! owns the files.
use super::support::*;
use crate::catalog::{
    self, CatalogContext, EntryKey, Index, SearchOptions, Source, SourceCatalog, synthetic,
};
use crate::library::bundle::{
    self, Bundle, Link, PackedAsset, RefMap, RefSite, RevisionRecord, SourceRevision, content_hash,
    references, rewrite_refs, validate_bundle,
};
use crate::library::import::{self, OtherRevision, PlaceSource, PromotePlan};
use crate::library::{Definition, Library};
use crate::updates::Change;
use crate::{
    ComponentRef, ContentHash, DefinitionId, DefinitionKind, DefinitionRef, Engine, Error,
    LibraryCommand, LibraryId, Operation, Outcome, Query, QueryResult, Result, Revision,
    SourceLineageId, format,
};
use std::collections::BTreeSet;
use std::sync::Arc;

const FOLDER: LibraryId = LibraryId(0x5eed_0000_0000_0000_0000_0000_0000_0001);
const OTHER: LibraryId = LibraryId(0x5eed_0000_0000_0000_0000_0000_0000_0002);
/// Generated libraries number their lineages from here, so any of these
/// numbers surviving in an imported copy is a reference the visitor missed.
const SOURCE_IDS: u64 = 9_000_000_000;

/// xorshift64*: a small deterministic generator for property tests.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn fresh(rng: &mut Rng, count: usize) -> Vec<SourceLineageId> {
    let mut out = BTreeSet::new();
    while out.len() < count {
        out.insert(SourceLineageId(
            (u128::from(rng.next()) << 64) | u128::from(rng.next()),
        ));
    }
    let mut out: Vec<_> = out.into_iter().collect();
    // Order is what a plan consumes; shuffle it.
    for i in (1..out.len()).rev() {
        out.swap(i, rng.below(i + 1));
    }
    out
}

/// Renumber every lineage of `library` through the visitor.
fn renumber(library: &mut Library, map: &dyn Fn(DefinitionId) -> DefinitionId) -> Result<()> {
    struct Map<'a>(&'a dyn Fn(DefinitionId) -> DefinitionId);
    impl RefMap for Map<'_> {
        fn exact(&mut self, target: DefinitionRef) -> Result<DefinitionRef> {
            Ok(DefinitionRef {
                id: (self.0)(target.id),
                ..target
            })
        }
        fn lineage(&mut self, _: DefinitionKind, id: DefinitionId) -> Result<DefinitionId> {
            Ok((self.0)(id))
        }
    }
    let mut out = Library {
        origins: library.origins.clone(),
        assets: library.assets.clone(),
        ..Library::default()
    };
    for r in library.revisions() {
        let (meta, mut body) = library
            .definition(r)
            .ok_or_else(|| Error::NotFound("revision".into()))?;
        rewrite_refs(&mut body, &mut Map(map))?;
        push(&mut out, map(r.id), r.revision, meta, body);
    }
    *library = out;
    Ok(())
}

fn push(
    library: &mut Library,
    id: DefinitionId,
    revision: Revision,
    meta: crate::library::DefinitionMeta,
    body: Definition,
) {
    use crate::library::Published;
    macro_rules! put {
        ($list:ident, $d:expr) => {
            library.$list.push(Published {
                id,
                revision,
                meta,
                definition: $d,
            })
        };
    }
    match body {
        Definition::Component(d) => put!(components, d),
        Definition::Symbol(d) => put!(symbols, d),
        Definition::Package(d) => put!(packages, d),
        Definition::Footprint(d) => put!(footprints, d),
        Definition::Binding(d) => put!(bindings, d),
        Definition::QuantityKind(d) => put!(quantity_kinds, d),
        Definition::Field(d) => put!(fields, d),
        Definition::Part(d) => put!(parts, *d),
        Definition::Model(d) => put!(models, *d),
        Definition::PackageRow(d) => put!(package_rows, *d),
    }
}

/// A publish-valid library of every kind, generated from `seed`: the
/// synthetic catalog renumbered from [`SOURCE_IDS`] in a random order, with
/// symbol and footprint texts that show fields, Part references with packed
/// attached copies, and second revisions of quantity kinds, fields and
/// components so closures hold two revisions of one lineage.
fn generated(seed: u64) -> Result<Library> {
    use crate::library::graphics::{Item, TextContent};
    let mut rng = Rng(seed | 1);
    let mut library = synthetic::project_with_parts(2 + rng.below(4))?.library;
    // Symbol texts show a field the owning component selects.
    let selected: Vec<(DefinitionId, DefinitionId)> = library
        .components
        .iter()
        .filter_map(|c| {
            let field = c.definition.fields.first()?.field.id;
            Some((c.id, field))
        })
        .collect();
    for symbol in &mut library.symbols {
        let Some((_, field)) = selected
            .iter()
            .find(|(component, _)| *component == symbol.definition.component)
        else {
            continue;
        };
        if !rng.chance(70) {
            continue;
        }
        for unit in &mut symbol.definition.units {
            for graphic in &mut unit.graphics {
                if let Item::Text(text) = &mut graphic.item
                    && text.content == TextContent::ComponentName
                {
                    text.content = TextContent::Field(*field);
                }
            }
        }
    }
    // Some footprints show a field next to the designator.
    let any_field = library.fields.first().map(|f| f.id);
    for footprint in &mut library.footprints {
        if let Some(field) = any_field
            && rng.chance(30)
        {
            for graphic in &mut footprint.definition.graphics {
                if let Item::Text(text) = &mut graphic.item
                    && text.content == TextContent::Designator
                {
                    let mut copy = graphic.clone();
                    if let Item::Text(t) = &mut copy.item {
                        t.content = TextContent::Field(field);
                    }
                    copy.id = crate::GraphicId(footprint.definition.next_local);
                    footprint.definition.next_local += 1;
                    footprint.definition.graphics.push(copy);
                    break;
                }
            }
        }
    }
    // Attached copies, packed by hash (D7).
    for part in &mut library.parts {
        if rng.chance(60) {
            let bytes: Vec<u8> = (0..1 + rng.below(64)).map(|_| rng.next() as u8).collect();
            let hash = bundle::asset_hash(&bytes);
            if let Some(reference) = part.definition.references.first_mut() {
                reference.asset = Some(hash.to_string());
            }
            if !library.assets.iter().any(|a| a.hash == hash) {
                library.assets.push(PackedAsset {
                    hash,
                    media_type: "application/pdf".into(),
                    data: bundle::AssetBytes(bytes),
                });
            }
        }
    }
    // Second revisions: a quantity kind r2, a new field on it and a
    // component r2 selecting that field beside one on r1, so its closure
    // holds two revisions of one lineage.
    let next_id = library
        .revisions()
        .iter()
        .map(|r| r.id.0)
        .max()
        .unwrap_or(0)
        + 1;
    if let Some(field) = library
        .fields
        .iter()
        .find(|f| f.definition.value_type.kind().is_some())
        .cloned()
        && let Some(kind_ref) = field.definition.value_type.kind()
        && let Some(kind) = library.quantity_kind(kind_ref).cloned()
        && let Some(mut c2) = library
            .components
            .iter()
            .find(|c| c.definition.fields.iter().any(|s| s.field.id == field.id))
            .cloned()
    {
        let mut kind2 = kind.clone();
        kind2.revision = Revision(2);
        kind2.meta.description = "Second revision.".into();
        library.quantity_kinds.push(kind2);
        let mut field2 = field.clone();
        field2.id = DefinitionId(next_id);
        field2.meta.name = format!("{} (revised kind)", field.meta.name);
        set_kind(
            &mut field2.definition.value_type,
            crate::QuantityKindRef::new(kind.id, Revision(2)),
        );
        library.fields.push(field2);
        c2.revision = Revision(2);
        c2.meta.description = "Second revision.".into();
        c2.definition.fields.push(crate::fields::FieldSelection {
            field: crate::FieldRef::new(DefinitionId(next_id), Revision::FIRST),
            default: None,
            show_on_symbol: false,
        });
        library.components.push(c2);
    }
    // A random renumbering far from any project counter.
    let ids: Vec<DefinitionId> = library
        .revisions()
        .iter()
        .map(|r| r.id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut order: Vec<usize> = (0..ids.len()).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i + 1));
    }
    let table: Vec<(DefinitionId, DefinitionId)> = ids
        .iter()
        .zip(order)
        .map(|(id, slot)| (*id, DefinitionId(SOURCE_IDS + 7 * slot as u64 + 3)))
        .collect();
    renumber(&mut library, &|id| {
        table
            .iter()
            .find(|(from, _)| *from == id)
            .map(|(_, to)| *to)
            .unwrap_or(id)
    })?;
    Ok(library)
}

fn set_kind(value_type: &mut crate::fields::ValueType, to: crate::QuantityKindRef) {
    use crate::fields::{ToleranceBasis, ValueType};
    match value_type {
        ValueType::Quantity { kind, .. }
        | ValueType::Range { kind }
        | ValueType::Tolerance {
            basis: ToleranceBasis::Absolute { kind },
        } => *kind = to,
        _ => {}
    }
}

/// Every integer of a JSON value at least [`SOURCE_IDS`].
fn source_ids(value: &serde_json::Value, out: &mut Vec<u64>) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(n) = n.as_u64()
                && n >= SOURCE_IDS
            {
                out.push(n);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| source_ids(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| source_ids(v, out)),
        _ => {}
    }
}

fn counter(start: u64) -> impl FnMut() -> Result<DefinitionId> {
    let mut next = start;
    move || {
        next += 1;
        Ok(DefinitionId(next))
    }
}

/// D1 property: for generated closures of every kind, export then import
/// into another project then export again gives equal records and hashes;
/// the imported copy passes the publish rules and holds no source lineage
/// id (a missed reference would survive the rewrite); a second import
/// reuses everything.
#[test]
fn import_then_export_round_trips_generated_closures_of_every_kind() -> Result<()> {
    let mut record_kinds = BTreeSet::new();
    let mut sites = BTreeSet::new();
    let mut multi_revision = false;
    let mut with_assets = false;
    let mut round_trips = 0usize;
    for seed in 1..=12u64 {
        let library = generated(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15))?;
        crate::library::validate(&library)?;
        let mut rng = Rng(seed);
        for root in library.revisions() {
            let ids = fresh(&mut rng, 64);
            let plan = match import::export(&library, root, FOLDER, "Generated", &ids) {
                Ok(plan) => plan,
                Err(Error::Invalid(message)) if message.contains("neither in its closure") => {
                    // A symbol, binding or field-showing footprint without
                    // its owner: refused, never exported dangling.
                    assert!(
                        matches!(
                            root.kind,
                            DefinitionKind::Symbol
                                | DefinitionKind::Binding
                                | DefinitionKind::Footprint
                        ),
                        "{root:?}: {message}"
                    );
                    continue;
                }
                Err(other) => return Err(other),
            };
            validate_bundle(&plan.bundle)?;
            assert_eq!(plan.origins.len(), plan.bundle.records.len());
            assert_eq!(
                plan.bundle.root,
                plan.bundle.records[plan.bundle.records.len() - 1].lineage_revision()
            );
            let lineages: BTreeSet<_> = plan.bundle.records.iter().map(|r| r.lineage).collect();
            assert_eq!(plan.used.len(), lineages.len(), "one fresh id per lineage");
            assert_eq!(
                &ids[..plan.used.len()],
                &plan.used[..],
                "fresh ids in order"
            );
            for record in &plan.bundle.records {
                record_kinds.insert(record.kind);
                with_assets |= !record.assets.is_empty();
                for site in references(&record.definition)? {
                    sites.insert(match site {
                        RefSite::Exact { target } => (target.kind, true),
                        RefSite::Lineage { kind, .. } => (kind, false),
                    });
                }
            }
            multi_revision |= lineages.len() < plan.bundle.records.len();
            // Deterministic.
            assert_eq!(
                import::export(&library, root, FOLDER, "Generated", &ids)?,
                plan
            );

            let mut target = Library::default();
            let mut allocate = counter(100 + rng.below(1000) as u64);
            let report = import::import(&mut target, &plan.bundle, &mut allocate)?;
            assert!(report.revisions.iter().all(|r| !r.reused));
            assert_eq!(target.revisions().len(), plan.bundle.records.len());
            assert_eq!(report.root.kind, root.kind);
            crate::library::validate(&target)?;
            let mut leaked = Vec::new();
            source_ids(&serde_json::to_value(&target)?, &mut leaked);
            assert!(
                leaked.is_empty(),
                "{root:?}: source ids survived: {leaked:?}"
            );

            let back = import::export(&target, report.root, FOLDER, "Generated", &[])?;
            assert_eq!(back.bundle, plan.bundle, "{root:?}");
            for (a, b) in back.bundle.records.iter().zip(&plan.bundle.records) {
                assert_eq!(content_hash(a)?, content_hash(b)?);
            }
            assert!(back.origins.is_empty() && back.used.is_empty());

            let before = target.clone();
            let again = import::import(&mut target, &plan.bundle, &mut allocate)?;
            assert!(again.revisions.iter().all(|r| r.reused));
            assert_eq!(again.root, report.root);
            assert_eq!(target, before, "re-importing an equal origin adds nothing");
            round_trips += 1;
        }
    }
    assert!(round_trips > 100, "{round_trips} round trips");
    for kind in [
        DefinitionKind::Component,
        DefinitionKind::Symbol,
        DefinitionKind::Package,
        DefinitionKind::Footprint,
        DefinitionKind::Binding,
        DefinitionKind::QuantityKind,
        DefinitionKind::Field,
        DefinitionKind::Part,
    ] {
        assert!(
            record_kinds.contains(&kind),
            "no {kind:?} record was generated"
        );
    }
    for site in [
        (DefinitionKind::Symbol, true),
        (DefinitionKind::Binding, true),
        (DefinitionKind::Field, true),
        (DefinitionKind::Package, true),
        (DefinitionKind::Footprint, true),
        (DefinitionKind::QuantityKind, true),
        (DefinitionKind::Component, true),
        (DefinitionKind::Component, false),
        (DefinitionKind::Field, false),
    ] {
        assert!(sites.contains(&site), "no {site:?} reference was generated");
    }
    assert!(
        multi_revision,
        "no closure held two revisions of one lineage"
    );
    assert!(with_assets, "no record named an attached copy");
    Ok(())
}

trait At {
    fn lineage_revision(&self) -> SourceRevision;
}

impl At for RevisionRecord {
    fn lineage_revision(&self) -> SourceRevision {
        SourceRevision {
            lineage: self.lineage,
            revision: self.revision,
        }
    }
}

#[test]
fn the_visitor_lists_references_in_a_fixed_order() -> Result<()> {
    let library = generated(7)?;
    let component = library
        .components
        .iter()
        .find(|c| !c.definition.fields.is_empty())
        .ok_or_else(|| Error::NotFound("component".into()))?;
    let sites = references(&Definition::Component(component.definition.clone()))?;
    let mut expected: Vec<RefSite> = Vec::new();
    expected.extend(
        component
            .definition
            .representations
            .iter()
            .map(|r| RefSite::Exact { target: r.erased() }),
    );
    expected.extend(
        component
            .definition
            .implementations
            .iter()
            .map(|r| RefSite::Exact { target: r.erased() }),
    );
    expected.extend(component.definition.fields.iter().map(|s| RefSite::Exact {
        target: s.field.erased(),
    }));
    assert_eq!(sites, expected);
    let part = library
        .parts
        .first()
        .ok_or_else(|| Error::NotFound("part".into()))?;
    let sites = references(&Definition::Part(Box::new(part.definition.clone())))?;
    assert_eq!(
        sites[0],
        RefSite::Exact {
            target: part.definition.component.erased()
        }
    );
    assert_eq!(
        sites[1],
        RefSite::Exact {
            target: part.definition.binding.erased()
        }
    );
    assert_eq!(sites.len(), 2 + part.definition.specs.len());
    Ok(())
}

#[test]
fn closures_are_dependency_first_and_refuse_dangling_references() -> Result<()> {
    let library = generated(3)?;
    let part = library
        .parts
        .first()
        .ok_or_else(|| Error::NotFound("part".into()))?;
    let root = DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    };
    let closure = bundle::closure(&library, root)?;
    assert_eq!(closure.last(), Some(&root));
    for (i, member) in closure.iter().enumerate() {
        let (_, body) = library
            .definition(*member)
            .ok_or_else(|| Error::NotFound("member".into()))?;
        for site in references(&body)? {
            if let RefSite::Exact { target } = site {
                let at = closure.iter().position(|m| *m == target);
                assert!(at.is_some_and(|at| at < i), "{target:?} before {member:?}");
            }
        }
    }
    let mut broken = library.clone();
    broken.bindings.clear();
    let error = bundle::closure(&broken, root).err();
    assert!(
        matches!(&error, Some(Error::NotFound(m)) if m.contains("names binding")),
        "{error:?}"
    );
    Ok(())
}

fn plan_of(library: &Library, root: DefinitionRef) -> Result<PromotePlan> {
    // New ids per state: ids the library already holds are refused.
    let seed = 99 + library.origins.len() as u64;
    import::export(
        library,
        root,
        FOLDER,
        "Shared parts",
        &fresh(&mut Rng(seed), 64),
    )
}

fn component_root(library: &Library) -> Result<DefinitionRef> {
    library
        .latest_components()
        .first()
        .map(|c| DefinitionRef {
            kind: DefinitionKind::Component,
            id: c.id,
            revision: c.revision,
        })
        .ok_or_else(|| Error::NotFound("component".into()))
}

fn rejected(bundle: &Bundle, expect: &str) -> Result<()> {
    match validate_bundle(bundle) {
        Err(Error::Invalid(message)) if message.contains(expect) => Ok(()),
        other => Err(Error::Invalid(format!("{expect}: {other:?}"))),
    }
}

#[test]
fn bundle_validation_refuses_dangling_noncanonical_and_unpacked_content() -> Result<()> {
    let library = generated(5)?;
    let plan = plan_of(&library, component_root(&library)?)?;
    validate_bundle(&plan.bundle)?;

    let mut missing = plan.bundle.clone();
    missing.records.remove(0);
    rejected(&missing, "does not carry")?;

    let mut twice = plan.bundle.clone();
    twice.records.push(twice.records[0].clone());
    rejected(&twice, "twice")?;

    let mut shuffled = plan.bundle.clone();
    let with_links = shuffled
        .records
        .iter_mut()
        .find(|r| r.links.len() >= 2)
        .ok_or_else(|| Error::NotFound("a record with two links".into()))?;
    let (a, b) = (with_links.links[0].lineage, with_links.links[1].lineage);
    with_links.links[0].lineage = b;
    with_links.links[1].lineage = a;
    // Swapped targets still resolve, but to the wrong kinds or revisions.
    assert!(validate_bundle(&shuffled).is_err());

    let mut renumbered = plan.bundle.clone();
    if let Some(record) = renumbered.records.iter_mut().find(|r| !r.links.is_empty()) {
        record.links.push(Link {
            local: DefinitionId(99),
            lineage: SourceLineageId(7),
        });
    }
    rejected(&renumbered, "not canonical")?;

    let mut wrong_kind = plan.bundle.clone();
    wrong_kind.records[0].kind = DefinitionKind::Part;
    rejected(&wrong_kind, "says Part")?;

    let mut extra = plan.bundle.clone();
    let bytes = b"unreferenced".to_vec();
    extra.assets.push(PackedAsset {
        hash: bundle::asset_hash(&bytes),
        media_type: "text/plain".into(),
        data: bundle::AssetBytes(bytes),
    });
    rejected(&extra, "no record names")?;

    let mut no_root = plan.bundle.clone();
    no_root.root.revision = Revision(77);
    rejected(&no_root, "root")?;
    Ok(())
}

#[test]
fn packed_assets_travel_by_hash_and_unpacked_ones_are_reported() -> Result<()> {
    let mut library = generated(11)?;
    let part = library
        .parts
        .iter()
        .find(|p| p.definition.references.iter().any(|r| r.asset.is_some()))
        .cloned()
        .ok_or_else(|| Error::NotFound("a part with an asset".into()))?;
    let root = DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    };
    let plan = plan_of(&library, root)?;
    let hash = ContentHash::parse(part.definition.references[0].asset.as_deref().unwrap_or(""))
        .ok_or_else(|| Error::Invalid("hash".into()))?;
    assert!(plan.bundle.assets.iter().any(|a| a.hash == hash));
    let mut target = Library::default();
    import::import(&mut target, &plan.bundle, &mut counter(10))?;
    assert!(
        target.assets.iter().any(|a| a.hash == hash),
        "packed into the project"
    );
    assert!(bundle::asset_findings(&target).is_empty());

    // Bytes that do not hash to their name are refused.
    let mut forged = plan.bundle.clone();
    if let Some(asset) = forged.assets.first_mut() {
        asset.data.0.push(0);
    }
    rejected(&forged, "content hash")?;

    // A project that lost the bytes reports it and cannot promote it.
    library.assets.retain(|a| a.hash != hash);
    let codes: Vec<String> = bundle::asset_findings(&library)
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert!(codes.iter().any(|c| c == "asset.missing"), "{codes:?}");
    assert!(matches!(plan_of(&library, root), Err(Error::NotFound(m)) if m.contains("not packed")));
    Ok(())
}

#[test]
fn export_maps_held_members_back_and_numbers_new_revisions_after_them() -> Result<()> {
    let mut library = generated(13)?;
    let root = component_root(&library)?;
    let first = plan_of(&library, root)?;
    library.origins.extend(first.origins.iter().copied());
    crate::library::bundle::validate_snapshot(&library)?;
    // Promoting again writes the same records and records nothing.
    let again = plan_of(&library, root)?;
    assert_eq!(again.bundle, first.bundle);
    assert!(again.origins.is_empty() && again.used.is_empty());
    // A new project revision of the root becomes the next source revision
    // of the same source lineage.
    let mut next = library
        .components
        .iter()
        .find(|c| c.id == root.id && c.revision == root.revision)
        .cloned()
        .ok_or_else(|| Error::NotFound("root".into()))?;
    next.revision = Revision(root.revision.0 + 1);
    next.meta.description = "Edited after promoting.".into();
    library.components.push(next);
    let root2 = DefinitionRef {
        revision: Revision(root.revision.0 + 1),
        ..root
    };
    let plan = plan_of(&library, root2)?;
    assert_eq!(plan.origins.len(), 1, "only the new revision is new");
    assert!(plan.used.is_empty());
    let source = first.bundle.root;
    assert_eq!(plan.bundle.root.lineage, source.lineage);
    assert_eq!(plan.bundle.root.revision, Revision(source.revision.0 + 1));
    // Fresh ids that the library already holds are refused.
    let held = vec![source.lineage];
    assert!(matches!(
        import::export(&library, root2, FOLDER, "Shared parts", &held),
        Err(Error::Invalid(m)) if m.contains("already a lineage")
    ));
    // The same revision may be promoted to a second library.
    let other = import::export(&library, root2, OTHER, "Other", &fresh(&mut Rng(5), 64))?;
    library.origins.extend(other.origins.iter().copied());
    crate::library::bundle::validate_snapshot(&library)?;
    Ok(())
}

fn engine_plan(
    engine: &Engine,
    root: DefinitionRef,
    ids: &[SourceLineageId],
) -> Result<PromotePlan> {
    match engine.query(Query::PromotePreview {
        root,
        library: FOLDER,
        name: "Shared parts".into(),
        fresh: ids.to_vec(),
    })? {
        QueryResult::Promote(plan) => Ok(*plan),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

fn promote(
    engine: &mut Engine,
    root: DefinitionRef,
    ids: &[SourceLineageId],
) -> Result<PromotePlan> {
    let plan = engine_plan(engine, root, ids)?;
    let outcome = apply(
        engine,
        Operation::Library(LibraryCommand::Promote {
            root,
            library: FOLDER,
            name: "Shared parts".into(),
            fresh: ids.to_vec(),
        }),
    )?;
    assert_eq!(
        outcome,
        Outcome::Promoted {
            origins: plan.origins.clone()
        }
    );
    Ok(plan)
}

/// io's write: keep records the folder lacks, never replace one.
fn write(folder: &mut Vec<RevisionRecord>, bundle: &Bundle) -> Result<()> {
    for record in &bundle.records {
        match folder
            .iter()
            .find(|r| r.lineage == record.lineage && r.revision == record.revision)
        {
            Some(held) if content_hash(held)? != content_hash(record)? => {
                return Err(Error::Conflict("rN changed".into()));
            }
            Some(_) => {}
            None => folder.push(record.clone()),
        }
    }
    Ok(())
}

fn bundle_of(folder: &[RevisionRecord], root: SourceRevision) -> Result<Bundle> {
    // The closure of `root` inside the folder, as io's read_bundle gives it.
    let mut wanted = vec![root];
    let mut records: Vec<RevisionRecord> = Vec::new();
    while let Some(at) = wanted.pop() {
        if records
            .iter()
            .any(|r| r.lineage == at.lineage && r.revision == at.revision)
        {
            continue;
        }
        let record = folder
            .iter()
            .find(|r| r.lineage == at.lineage && r.revision == at.revision)
            .cloned()
            .ok_or_else(|| Error::NotFound("record".into()))?;
        for site in references(&record.definition)? {
            if let RefSite::Exact { target } = site
                && let Some(link) = record.links.iter().find(|l| l.local == target.id)
            {
                wanted.push(SourceRevision {
                    lineage: link.lineage,
                    revision: target.revision,
                });
            }
        }
        records.push(record);
    }
    records.sort_by_key(|r| (r.kind == DefinitionKind::Component, r.lineage, r.revision));
    Ok(Bundle {
        library: FOLDER,
        name: "Shared parts".into(),
        root,
        records,
        assets: Vec::new(),
    })
}

fn place_from(
    engine: &mut Engine,
    bundle: &Bundle,
    other_revision: OtherRevision,
) -> Result<Outcome> {
    apply(
        engine,
        Operation::Library(LibraryCommand::PlaceFrom {
            source: PlaceSource::Bundle {
                bundle: Box::new(bundle.clone()),
            },
            position: [0.0, 0.0],
            designator: None,
            other_revision,
        }),
    )
}

fn updates(engine: &Engine, folder: &[RevisionRecord]) -> Result<Vec<catalog::UpdateAvailable>> {
    let source = Arc::new(catalog::source_catalog(FOLDER, "Shared parts", folder)?);
    let context = CatalogContext {
        sources: vec![source],
        ..CatalogContext::default()
    };
    match engine.query_with(Query::UpdatesAvailable {}, &context)? {
        QueryResult::UpdatesAvailable(list) => Ok(list),
        other => Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
}

/// The S3 scenario in the core: promote into a folder, place from it in
/// another project, publish r+1 in the folder, see the update, refuse a
/// silent upgrade, update only R1, then drop the folder and reopen the
/// project offline.
#[test]
fn place_from_a_folder_then_update_only_r1_and_reopen_offline() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let ids = fresh(&mut Rng(42), 16);
    let mut folder: Vec<RevisionRecord> = Vec::new();
    let first = promote(&mut author, root, &ids)?;
    write(&mut folder, &first.bundle)?;
    assert_eq!(author.project().library.origins, first.origins);
    // Promoting again records nothing and takes no undo step.
    let revision = author.revision();
    promote(&mut author, root, &fresh(&mut Rng(43), 16))?;
    assert_eq!(author.revision(), revision);

    // Install and place, twice: the second reuses the copy.
    let mut design = Engine::default();
    let placed = place_from(&mut design, &first.bundle, OtherRevision::Refuse)?;
    let Outcome::PlacedFrom {
        import: Some(report),
        instance: r1,
        designator,
        ..
    } = placed
    else {
        return Err(Error::Invalid(format!("unexpected {placed:?}")));
    };
    assert_eq!(designator, "R1");
    assert_eq!(report.revisions.len(), first.bundle.records.len());
    let copy = ComponentRef::new(report.root.id, report.root.revision);
    let Outcome::PlacedFrom {
        import: Some(again),
        instance: r2,
        designator,
        ..
    } = place_from(&mut design, &first.bundle, OtherRevision::Refuse)?
    else {
        return Err(Error::Invalid("second place".into()));
    };
    assert_eq!(designator, "R2");
    assert!(again.revisions.iter().all(|r| r.reused));
    assert_eq!(
        design.project().library.revisions().len(),
        first.bundle.records.len()
    );
    assert!(
        updates(&design, &folder)?.is_empty(),
        "the folder holds nothing newer"
    );

    // Publish r+1 of the component in the folder (the demo keeps its
    // drafts open).
    let mut body = body(&author, root.id)?;
    if let Definition::Component(c) = &mut body {
        c.designator_prefix = "R".into();
    }
    let draft = author
        .drafts()
        .get(root.id)
        .cloned()
        .ok_or_else(|| Error::NotFound("draft".into()))?;
    let mut meta = draft.meta.clone();
    meta.description = "Revised in the folder.".into();
    edit(
        &mut author,
        vec![crate::library::DraftEdit {
            id: root.id,
            meta,
            definition: body,
        }],
    )?;
    let revised = publish(&mut author, vec![root.id])?;
    let root2 = revised[0];
    let second = promote(&mut author, root2, &ids[first.used.len()..])?;
    assert_eq!(second.origins.len(), 1);
    write(&mut folder, &second.bundle)?;
    let source_root = second.bundle.root;
    assert_eq!(source_root.lineage, first.bundle.root.lineage);
    assert_eq!(source_root.revision, Revision(2));

    // Update available, marked on the project's catalog entry too.
    let found = updates(&design, &folder)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].local, copy.erased());
    assert_eq!(found[0].latest, Revision(2));
    assert!(found[0].modified.is_empty());
    let codes: Vec<String> = catalog::update_findings(&design.project().library, &found)
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert_eq!(codes, vec!["library.update_available".to_owned()]);
    let source = Arc::new(catalog::source_catalog(FOLDER, "Shared parts", &folder)?);
    let index = Index::with_sources(
        design.project(),
        design.drafts(),
        std::slice::from_ref(&source),
    )?;
    let mine = index
        .entries()
        .iter()
        .find(|e| e.source == Source::Project && e.lineage == copy.id)
        .ok_or_else(|| Error::NotFound("project entry".into()))?;
    assert_eq!(mine.update.as_ref().map(|u| u.latest), Some(Revision(2)));
    let theirs = index
        .entries()
        .iter()
        .find(|e| e.source == Source::Library(FOLDER))
        .ok_or_else(|| Error::NotFound("folder entry".into()))?;
    assert_eq!(theirs.revision, Some(Revision(2)));
    assert_eq!(
        theirs.key(),
        EntryKey::Library {
            library: FOLDER,
            lineage: source_root.lineage
        }
    );
    assert_eq!(
        theirs.component(),
        None,
        "folder entries place through place_from"
    );

    // Never upgraded silently: refused, nothing changes.
    let bundle2 = bundle_of(&folder, source_root)?;
    let kept = Snapshot::of(&design);
    let refused = place_from(&mut design, &bundle2, OtherRevision::Refuse);
    assert!(
        refused.as_ref().is_err_and(import::is_other_revision),
        "{refused:?}"
    );
    kept.assert_kept(&design, "other revision refused");

    // Update project copy: import r2, then update only R1 through the
    // reviewed update path.
    let Outcome::Imported { report } = apply(
        &mut design,
        Operation::Library(LibraryCommand::Import {
            bundle: Box::new(bundle2.clone()),
        }),
    )?
    else {
        return Err(Error::Invalid("import".into()));
    };
    let target = ComponentRef::new(report.root.id, report.root.revision);
    assert_eq!(target.id, copy.id, "the same project lineage");
    assert_eq!(target.revision, Revision(2));
    assert_eq!(
        report.revisions.iter().filter(|r| !r.reused).count(),
        1,
        "only the new revision is appended"
    );
    let change = Change::ComponentRevision {
        instances: vec![r1],
        target,
        resolutions: Default::default(),
    };
    assert!(design.preview_update(change.clone()).blockers.is_empty());
    design.apply_update(design.revision(), change, false)?;
    let pinned = |engine: &Engine, id| engine.project().instance(id).map(|i| i.component);
    assert_eq!(pinned(&design, r1), Some(target));
    assert_eq!(pinned(&design, r2), Some(copy));
    assert!(updates(&design, &folder)?.is_empty());

    // Delete the folder and reopen offline: nothing resolves through it.
    drop(folder);
    let bytes = format::encode(&design.to_file())?;
    let reopened = Engine::from_file(format::decode(&bytes)?)?;
    assert_eq!(reopened.project(), design.project());
    assert_eq!(pinned(&reopened, r1), Some(target));
    assert_eq!(pinned(&reopened, r2), Some(copy));
    assert_eq!(
        reopened.project().library.origins.len(),
        first.bundle.records.len() + 1
    );
    match reopened.query(Query::UpdatesAvailable {})? {
        QueryResult::UpdatesAvailable(list) => assert!(list.is_empty()),
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
    Ok(())
}

#[test]
fn keep_both_places_the_new_revision_beside_the_old() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let ids = fresh(&mut Rng(8), 16);
    let first = promote(&mut author, root, &ids)?;
    let mut design = Engine::default();
    place_from(&mut design, &first.bundle, OtherRevision::Refuse)?;
    let revised = publish(&mut author, vec![root.id])?;
    let second = promote(&mut author, revised[0], &ids[first.used.len()..])?;
    let mut folder = Vec::new();
    write(&mut folder, &first.bundle)?;
    write(&mut folder, &second.bundle)?;
    let bundle2 = bundle_of(&folder, second.bundle.root)?;
    let Outcome::PlacedFrom {
        instance,
        designator,
        ..
    } = place_from(&mut design, &bundle2, OtherRevision::KeepBoth)?
    else {
        return Err(Error::Invalid("keep both".into()));
    };
    assert_eq!(designator, "R2");
    let component = design.project().instance(instance).map(|i| i.component);
    assert_eq!(component.map(|c| c.revision), Some(Revision(2)));
    // One undo step removes the import and the instance together.
    let before = design.project().library.revisions().len();
    design.undo()?;
    assert!(design.project().instance(instance).is_none());
    assert!(design.project().library.revisions().len() < before);
    Ok(())
}

#[test]
fn a_modified_folder_record_is_detected_by_hash_and_never_trusted() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let plan = promote(&mut author, root, &fresh(&mut Rng(3), 16))?;
    let mut design = Engine::default();
    place_from(&mut design, &plan.bundle, OtherRevision::Refuse)?;
    // Someone edits r1 in place in the folder.
    let mut tampered = plan.bundle.clone();
    let record = tampered
        .records
        .iter_mut()
        .find(|r| r.lineage == plan.bundle.root.lineage)
        .ok_or_else(|| Error::NotFound("root record".into()))?;
    record.meta.description = "Edited in place.".into();
    let found = updates(&design, &tampered.records)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].modified, vec![Revision(1)], "the changed revision");
    assert_eq!(found[0].latest, Revision(1));
    let codes: Vec<String> = catalog::update_findings(&design.project().library, &found)
        .into_iter()
        .map(|f| f.code)
        .collect();
    assert_eq!(codes, vec!["library.source_modified".to_owned()]);
    let kept = Snapshot::of(&design);
    let error = apply(
        &mut design,
        Operation::Library(LibraryCommand::Import {
            bundle: Box::new(tampered),
        }),
    );
    assert!(
        matches!(&error, Err(Error::Conflict(m)) if m.starts_with(import::SOURCE_MODIFIED)),
        "{error:?}"
    );
    kept.assert_kept(&design, "modified record refused");
    Ok(())
}

#[test]
fn place_from_a_part_assigns_it() -> Result<()> {
    let library = generated(17)?;
    let part = library
        .parts
        .first()
        .cloned()
        .ok_or_else(|| Error::NotFound("part".into()))?;
    let root = DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    };
    let plan = plan_of(&library, root)?;
    let mut design = Engine::default();
    let Outcome::PlacedFrom {
        import: Some(report),
        instance,
        ..
    } = place_from(&mut design, &plan.bundle, OtherRevision::Refuse)?
    else {
        return Err(Error::Invalid("placed".into()));
    };
    let placed = design
        .project()
        .instance(instance)
        .ok_or_else(|| Error::NotFound("instance".into()))?;
    assert_eq!(
        placed.part.map(|p| p.erased()),
        Some(report.root),
        "the Part is assigned"
    );
    Ok(())
}

#[test]
fn duplicate_copies_a_component_with_its_symbols_and_bindings_as_drafts() -> Result<()> {
    let mut engine = Engine::demo()?;
    let root = component_root(&engine.project().library)?;
    let original = engine
        .project()
        .library
        .component(ComponentRef::new(root.id, root.revision))
        .cloned()
        .ok_or_else(|| Error::NotFound("component".into()))?;
    let Outcome::Duplicated { lineages } = apply(
        &mut engine,
        Operation::Library(LibraryCommand::Duplicate {
            component: ComponentRef::new(root.id, root.revision),
            name: "Resistor (copy)".into(),
        }),
    )?
    else {
        return Err(Error::Invalid("duplicate".into()));
    };
    let expected =
        1 + original.definition.representations.len() + original.definition.implementations.len();
    assert_eq!(lineages.len(), expected);
    let copy = engine
        .drafts()
        .get(lineages[0])
        .cloned()
        .ok_or_else(|| Error::NotFound("copy".into()))?;
    assert_eq!(copy.meta.name, "Resistor (copy)");
    assert!(copy.meta.provenance.contains("Duplicated from"));
    let Definition::Component(body) = &copy.definition else {
        return Err(Error::Invalid("kind".into()));
    };
    assert_eq!(
        body.fields, original.definition.fields,
        "fields stay shared"
    );
    for id in &lineages[1..] {
        match engine.drafts().get(*id).map(|d| &d.definition) {
            Some(Definition::Symbol(s)) => assert_eq!(s.component, lineages[0]),
            Some(Definition::Binding(b)) => assert_eq!(b.component, lineages[0]),
            other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
        }
    }
    let published = publish(&mut engine, lineages.clone())?;
    assert_eq!(published.len(), lineages.len());
    assert!(
        engine
            .project()
            .library
            .component(ComponentRef::new(lineages[0], Revision::FIRST))
            .is_some()
    );
    Ok(())
}

fn synthetic_folder() -> Result<Vec<RevisionRecord>> {
    let mut library = synthetic::project_with_parts(4)?.library;
    let mut folder = Vec::new();
    let mut rng = Rng(21);
    let roots: Vec<DefinitionRef> = library
        .latest_components()
        .iter()
        .map(|c| DefinitionRef {
            kind: DefinitionKind::Component,
            id: c.id,
            revision: c.revision,
        })
        .chain(library.latest_parts().iter().map(|p| DefinitionRef {
            kind: DefinitionKind::Part,
            id: p.id,
            revision: p.revision,
        }))
        .collect();
    for root in roots {
        let plan = import::export(&library, root, FOLDER, "Shared parts", &fresh(&mut rng, 64))?;
        library.origins.extend(plan.origins.iter().copied());
        write(&mut folder, &plan.bundle)?;
    }
    Ok(folder)
}

#[test]
fn folder_catalogs_are_deterministic_and_search_with_the_project() -> Result<()> {
    let folder = synthetic_folder()?;
    let catalog = catalog::source_catalog(FOLDER, "Shared parts", &folder)?;
    let mut reversed = folder.clone();
    reversed.reverse();
    let rebuilt = catalog::source_catalog(FOLDER, "Shared parts", &reversed)?;
    assert_eq!(rebuilt.lineages, catalog.lineages);
    assert_eq!(rebuilt.revisions, catalog.revisions);
    assert_eq!(rebuilt.fields, catalog.fields);
    assert_eq!(rebuilt.entries.len(), catalog.entries.len());
    for (a, b) in rebuilt.entries.iter().zip(&catalog.entries) {
        assert_eq!(a, b);
    }
    assert_eq!(rebuilt, catalog, "a rebuilt index is identical");
    assert_eq!(catalog.revisions.len(), folder.len());
    assert!(
        catalog
            .entries
            .iter()
            .all(|e| e.source == Source::Library(FOLDER)
                && e.source_lineage.is_some()
                && e.component().is_none()
                && e.part().is_none())
    );
    let parts = catalog
        .entries
        .iter()
        .filter(|e| e.kind == DefinitionKind::Part)
        .count();
    assert!(parts > 0);
    // Serializable, so io can cache it.
    let json = serde_json::to_string(&catalog)?;
    let back: SourceCatalog = serde_json::from_str(&json)?;
    assert_eq!(back, catalog);

    // Search the demo project with the folder: a field predicate reaches
    // folder entries through the merged vocabulary, and a field name held
    // by both the project and the folder is one key.
    let design = Engine::demo()?;
    let source = Arc::new(catalog);
    let index = Index::with_sources(
        design.project(),
        design.drafts(),
        std::slice::from_ref(&source),
    )?;
    let query = index
        .parse("res>=10k")
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let found = index.search(&query, &CatalogContext::default(), SearchOptions::default());
    assert!(
        found
            .results
            .iter()
            .any(|h| h.entry.source == Source::Library(FOLDER)),
        "{:?}",
        found.chips
    );
    let named = index
        .vocabulary()
        .fields
        .iter()
        .filter(|f| f.name == "Resistance")
        .count();
    assert!(
        named >= 2,
        "the project's and the folder's Resistance: {named}"
    );
    // Favourites are keyed by source identity, never by catalog-local ids.
    let folder_hit = found
        .results
        .iter()
        .find(|h| h.entry.source == Source::Library(FOLDER))
        .map(|h| h.entry.key())
        .ok_or_else(|| Error::NotFound("hit".into()))?;
    let context = CatalogContext {
        favourites: [folder_hit].into(),
        ..CatalogContext::default()
    };
    let ranked = index.search(&query, &context, SearchOptions::default());
    assert!(
        ranked
            .results
            .iter()
            .any(|h| h.favourite && h.entry.key() == folder_hit)
    );
    // The engine query reaches folders through the context.
    let context = CatalogContext {
        sources: vec![source],
        ..CatalogContext::default()
    };
    match design.query_with(
        Query::Search {
            query: "EX-".into(),
            limit: Some(500),
            include_drafts: false,
        },
        &context,
    )? {
        QueryResult::Search(report) => assert!(
            report
                .results
                .iter()
                .any(|h| h.entry.source == Source::Library(FOLDER))
        ),
        other => return Err(Error::Invalid(format!("unexpected {other:?}"))),
    }
    Ok(())
}

#[test]
fn origins_keep_project_and_source_lineages_one_to_one() -> Result<()> {
    let library = generated(19)?;
    let plan = plan_of(&library, component_root(&library)?)?;
    let mut held = library.clone();
    held.origins.extend(plan.origins.iter().copied());
    bundle::validate_snapshot(&held)?;
    let mut twice = held.clone();
    twice.origins.push(plan.origins[0]);
    assert!(bundle::validate_snapshot(&twice).is_err());
    // Two project lineages claiming one source lineage.
    let mut crossed = held.clone();
    let (first, other) = {
        let first = crossed.origins[0];
        let other = crossed
            .origins
            .iter()
            .position(|o| o.local.id != first.local.id)
            .ok_or_else(|| Error::NotFound("two lineages".into()))?;
        (first, other)
    };
    crossed.origins[other].origin.lineage = first.origin.lineage;
    crossed.origins[other].origin.revision = Revision(40);
    assert!(bundle::validate_snapshot(&crossed).is_err());
    Ok(())
}

fn import_bundle(engine: &mut Engine, bundle: &Bundle) -> Result<Outcome> {
    apply(
        engine,
        Operation::Library(LibraryCommand::Import {
            bundle: Box::new(bundle.clone()),
        }),
    )
}

fn is_older(result: &Result<Outcome>) -> bool {
    matches!(result, Err(Error::Conflict(m)) if m.starts_with(import::OLDER_REVISION))
}

/// Project revisions are numbered in import order, so importing source r1
/// after r2 would make the older content the project's latest. It is
/// refused, directly, as Keep both and as a dependency of another bundle,
/// and the document is unchanged.
#[test]
fn an_older_source_revision_is_never_imported_after_a_newer_one() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let ids = fresh(&mut Rng(21), 16);
    let first = promote(&mut author, root, &ids)?;
    let revised = publish(&mut author, vec![root.id])?;
    let second = promote(&mut author, revised[0], &ids[first.used.len()..])?;
    let mut folder = Vec::new();
    write(&mut folder, &first.bundle)?;
    write(&mut folder, &second.bundle)?;
    let bundle2 = bundle_of(&folder, second.bundle.root)?;

    let mut design = Engine::default();
    place_from(&mut design, &bundle2, OtherRevision::Refuse)?;
    let kept = Snapshot::of(&design);
    let direct = import_bundle(&mut design, &first.bundle);
    assert!(is_older(&direct), "{direct:?}");
    kept.assert_kept(&design, "older root refused");
    let both = place_from(&mut design, &first.bundle, OtherRevision::KeepBoth);
    assert!(is_older(&both), "{both:?}");
    kept.assert_kept(&design, "older keep-both refused");

    // A dependency: the project holds r2 of a record that another bundle
    // still pins at r1.
    let mut design = Engine::default();
    let leaf = first
        .bundle
        .records
        .iter()
        .find(|r| r.links.is_empty() && r.lineage != first.bundle.root.lineage)
        .cloned()
        .ok_or_else(|| Error::NotFound("a record without links".into()))?;
    let mut newer = leaf.clone();
    newer.revision = Revision(2);
    newer.meta.description = "Revised in the folder.".into();
    let newer_bundle = Bundle {
        library: FOLDER,
        name: "Shared parts".into(),
        root: SourceRevision {
            lineage: newer.lineage,
            revision: newer.revision,
        },
        records: vec![newer],
        assets: Vec::new(),
    };
    import_bundle(&mut design, &newer_bundle)?;
    let kept = Snapshot::of(&design);
    let dependency = place_from(&mut design, &first.bundle, OtherRevision::Refuse);
    assert!(is_older(&dependency), "{dependency:?}");
    kept.assert_kept(&design, "older dependency refused");
    // Imported in order (r1 first), both revisions are taken.
    let mut ordered = Engine::default();
    place_from(&mut ordered, &first.bundle, OtherRevision::Refuse)?;
    import_bundle(&mut ordered, &newer_bundle)?;
    Ok(())
}

/// Re-importing a bundle whose revisions are all in the project changes
/// nothing: no revision, no undo step.
#[test]
fn a_fully_reused_import_takes_no_revision_or_undo_step() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let plan = promote(&mut author, root, &fresh(&mut Rng(31), 16))?;
    let mut design = Engine::default();
    place_from(&mut design, &plan.bundle, OtherRevision::Refuse)?;
    prime_history(&mut design)?;
    let kept = Snapshot::of(&design);
    let Outcome::Imported { report } = import_bundle(&mut design, &plan.bundle)? else {
        return Err(Error::Invalid("import".into()));
    };
    assert!(report.revisions.iter().all(|r| r.reused));
    kept.assert_kept(&design, "fully reused import");
    Ok(())
}

/// The bundle of Part `index` of `library` with its first reference's
/// attached copy replaced by `data`.
fn part_with_asset(library: &Library, index: usize, data: Vec<u8>) -> Result<Bundle> {
    let part = library
        .parts
        .get(index)
        .ok_or_else(|| Error::NotFound("a part".into()))?;
    let root = DefinitionRef {
        kind: DefinitionKind::Part,
        id: part.id,
        revision: part.revision,
    };
    // Separate fresh ids per bundle: two exports of one library.
    let seed = 500 + u64::try_from(index).unwrap_or(0);
    let mut plan = import::export(
        library,
        root,
        FOLDER,
        "Shared parts",
        &fresh(&mut Rng(seed), 64),
    )?;
    let asset = PackedAsset {
        hash: bundle::asset_hash(&data),
        media_type: "application/pdf".into(),
        data: bundle::AssetBytes(data),
    };
    let record = plan
        .bundle
        .records
        .iter_mut()
        .find(|r| r.lineage == plan.bundle.root.lineage)
        .ok_or_else(|| Error::NotFound("part record".into()))?;
    let Definition::Part(body) = &mut record.definition else {
        return Err(Error::Invalid("the root is not a part".into()));
    };
    let reference = body
        .references
        .first_mut()
        .ok_or_else(|| Error::NotFound("a reference".into()))?;
    reference.asset = Some(asset.hash.to_string());
    record.assets = bundle::named_assets(&record.definition)?;
    let mut named = BTreeSet::new();
    for r in &plan.bundle.records {
        named.extend(bundle::named_assets(&r.definition)?);
    }
    let mut assets: Vec<PackedAsset> = plan
        .bundle
        .assets
        .iter()
        .filter(|a| named.contains(&a.hash))
        .cloned()
        .collect();
    assets.push(asset);
    assets.sort_by_key(|a| a.hash);
    plan.bundle.assets = assets;
    validate_bundle(&plan.bundle)?;
    Ok(plan.bundle)
}

/// Packed assets are bounded in total, as base64, so imports can never
/// leave a project too large to save: the second of two imports with a
/// 6 MiB attached copy each is refused and changes nothing.
#[test]
fn an_over_budget_import_is_refused_and_leaves_the_document_unchanged() -> Result<()> {
    let library = generated(17)?;
    let big = |seed: u8| -> Vec<u8> {
        (0..bundle::MAX_ASSET_BYTES / 4 * 3)
            .map(|i| u8::try_from(i % 251).unwrap_or(0) ^ seed)
            .collect()
    };
    assert!(
        bundle::encoded_len(bundle::MAX_ASSET_BYTES / 4 * 3) * 2 > bundle::MAX_PACKED_BYTES,
        "the fixture exceeds the budget"
    );
    let first = part_with_asset(&library, 0, big(1))?;
    let second = part_with_asset(&library, 1, big(2))?;
    let mut design = Engine::default();
    import_bundle(&mut design, &first)?;
    let kept = Snapshot::of(&design);
    let refused = import_bundle(&mut design, &second);
    assert!(matches!(&refused, Err(Error::Limit(_))), "{refused:?}");
    kept.assert_kept(&design, "over-budget import");
    // One bundle carrying both is refused by bundle validation alone.
    let mut both = second.clone();
    both.assets.extend(first.assets.iter().cloned());
    both.assets.sort_by_key(|a| a.hash);
    both.assets.dedup_by_key(|a| a.hash);
    assert!(matches!(
        bundle::validate_assets(&both.assets),
        Err(Error::Limit(_))
    ));
    Ok(())
}

/// The modified mark names the revision whose record changed, not the
/// newest one the project copied.
#[test]
fn a_modified_older_record_is_named_by_its_revision() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let ids = fresh(&mut Rng(9), 16);
    let first = promote(&mut author, root, &ids)?;
    let revised = publish(&mut author, vec![root.id])?;
    let second = promote(&mut author, revised[0], &ids[first.used.len()..])?;
    let mut folder = Vec::new();
    write(&mut folder, &first.bundle)?;
    write(&mut folder, &second.bundle)?;
    let mut design = Engine::default();
    place_from(&mut design, &first.bundle, OtherRevision::Refuse)?;
    import_bundle(&mut design, &bundle_of(&folder, second.bundle.root)?)?;
    assert!(updates(&design, &folder)?.is_empty());
    let record = folder
        .iter_mut()
        .find(|r| r.lineage == first.bundle.root.lineage && r.revision == Revision(1))
        .ok_or_else(|| Error::NotFound("r1".into()))?;
    record.meta.description = "Edited in place.".into();
    let found = updates(&design, &folder)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].origin.revision, Revision(2), "the newest copy");
    assert_eq!(found[0].modified, vec![Revision(1)], "the edited record");
    let finding = catalog::update_findings(&design.project().library, &found);
    assert!(
        finding
            .iter()
            .any(|f| f.code == "library.source_modified" && f.message.starts_with("r1 of")),
        "{finding:?}"
    );
    Ok(())
}

/// A project copy of a folder item is remembered per user by its origin,
/// so a favourite or recent names the same item in every project; a
/// project-only item has no per-user identity.
#[test]
fn project_copies_are_keyed_per_user_by_their_origin() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let plan = promote(&mut author, root, &fresh(&mut Rng(77), 16))?;
    let mut design = Engine::default();
    place_from(&mut design, &plan.bundle, OtherRevision::Refuse)?;
    let index = Index::of_project(design.project(), design.drafts())?;
    let copy = index
        .entries()
        .iter()
        .find(|e| e.kind == DefinitionKind::Component)
        .ok_or_else(|| Error::NotFound("project copy".into()))?;
    let library_key = EntryKey::Library {
        library: FOLDER,
        lineage: plan.bundle.root.lineage,
    };
    assert_eq!(copy.user_key(), Some(library_key));
    assert_eq!(copy.origin.map(|o| o.revision), Some(Revision(1)));
    assert!(copy.named_by(&BTreeSet::from([library_key])));
    // The author's own components were promoted, so they have an origin too;
    // a fresh project's own component has none.
    let local = Engine::demo()?;
    let index = Index::of_project(local.project(), local.drafts())?;
    assert!(
        index
            .entries()
            .iter()
            .all(|e| e.source == Source::Project && e.user_key().is_none())
    );
    Ok(())
}

fn edit_copy(engine: &mut Engine, bundle: &Bundle) -> Result<Outcome> {
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::EditCopy {
            bundle: Box::new(bundle.clone()),
        }),
    )
}

/// "Edit a copy in this project" is one command: the import and the
/// working-set drafts undo and redo as one step on every transport, a
/// repeat changes nothing, and a refused copy leaves the document,
/// revision and both histories alone.
#[test]
fn edit_a_copy_imports_and_drafts_in_one_undo_step() -> Result<()> {
    let mut author = Engine::demo()?;
    let root = component_root(&author.project().library)?;
    let plan = promote(&mut author, root, &fresh(&mut Rng(41), 16))?;
    let mut design = Engine::demo()?;
    prime_history(&mut design)?;
    let before = design.to_file();
    let revision = design.revision();
    let Outcome::Copied {
        component,
        import,
        drafted,
    } = edit_copy(&mut design, &plan.bundle)?
    else {
        return Err(Error::Invalid("copied".into()));
    };
    assert_eq!(design.revision(), revision + 1, "one step");
    assert_eq!(import.root.kind, DefinitionKind::Component);
    assert_eq!(drafted.first(), Some(&component.id), "component first");
    assert!(drafted.len() >= 4, "{drafted:?}");
    for id in &drafted {
        assert!(design.drafts().get(*id).is_some());
    }
    let after = design.to_file();
    design.undo()?;
    assert_eq!(
        design.to_file(),
        before,
        "one undo removes import and drafts"
    );
    design.redo()?;
    assert_eq!(design.to_file(), after, "one redo restores both");
    // Copying again reuses every revision and drafts nothing new.
    let again = design.revision();
    let Outcome::Copied { drafted, .. } = edit_copy(&mut design, &plan.bundle)? else {
        return Err(Error::Invalid("copied".into()));
    };
    assert!(drafted.is_empty());
    assert_eq!(design.revision(), again, "a repeat takes no step");
    // A bundle whose root is not a component or Part is refused whole.
    let package = plan
        .bundle
        .records
        .iter()
        .find(|r| r.kind == DefinitionKind::Package)
        .ok_or_else(|| Error::NotFound("package record".into()))?;
    let mut wrong = bundle_of(
        &plan.bundle.records,
        SourceRevision {
            lineage: package.lineage,
            revision: package.revision,
        },
    )?;
    wrong.library = plan.bundle.library;
    let mut fresh_design = Engine::demo()?;
    prime_history(&mut fresh_design)?;
    let kept = Snapshot::of(&fresh_design);
    assert!(edit_copy(&mut fresh_design, &wrong).is_err());
    kept.assert_kept(&fresh_design, "refused edit copy");
    Ok(())
}

/// `library.draft_closure` drafts the working set once, as one step.
#[test]
fn draft_closure_drafts_the_working_set_in_one_step() -> Result<()> {
    let mut engine = Engine::demo()?;
    let root = component_root(&engine.project().library)?;
    let component = crate::ids::ComponentRef::new(root.id, root.revision);
    for draft in engine.drafts().items.clone() {
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::DiscardDraft { id: draft.id }),
        )?;
    }
    let before = engine.to_file();
    let revision = engine.revision();
    let close = |engine: &mut Engine| {
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::DraftClosure { component }),
        )
    };
    let Outcome::DraftsCreated { ids } = close(&mut engine)? else {
        return Err(Error::Invalid("drafts".into()));
    };
    assert_eq!(ids.first(), Some(&component.id));
    assert!(ids.len() >= 4, "{ids:?}");
    assert_eq!(engine.revision(), revision + 1);
    let Outcome::DraftsCreated { ids: none } = close(&mut engine)? else {
        return Err(Error::Invalid("drafts".into()));
    };
    assert!(none.is_empty());
    assert_eq!(engine.revision(), revision + 1, "nothing left to draft");
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    Ok(())
}
