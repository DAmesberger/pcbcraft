//! The container round-trips projects with assets above the old 16 MiB
//! file bound, writes reproducible bytes, and refuses every malformed,
//! oversized or unknown entry while naming it.
use super::*;
use sparksmith_core::Engine;
use sparksmith_core::library::PackedAsset;
use sparksmith_core::library::bundle::asset_hash;

/// Deterministic, incompressible bytes.
pub(crate) fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

pub(crate) fn packed(bytes: Vec<u8>, media_type: &str) -> PackedAsset {
    PackedAsset {
        hash: asset_hash(&bytes),
        media_type: media_type.into(),
        data: AssetBytes::new(bytes),
    }
}

/// The demo project with `assets` packed, sorted by hash.
pub(crate) fn with_assets(assets: Vec<PackedAsset>) -> Result<EditorFile> {
    let mut file = Engine::demo()?.to_file();
    file.project.library.assets = assets.into();
    file.project.library.assets.sort_by_key(|a| a.hash);
    file.validate()?;
    Ok(file)
}

/// A raw archive of `entries`, written without any of the container's checks.
pub(crate) fn raw_zip(path: &Path, entries: &[(&str, &[u8], CompressionMethod)]) -> Result<()> {
    let mut zip = ZipWriter::new(File::create(path)?);
    for (name, bytes, method) in entries {
        zip.start_file(*name, options(*method)).map_err(malformed)?;
        zip.write_all(bytes)?;
    }
    zip.finish().map_err(malformed)?;
    Ok(())
}

/// The container's document of `file`.
fn document(file: &EditorFile) -> Result<Vec<u8>> {
    Ok(format::encode_container_document(file)?)
}

fn entry_names(path: &Path) -> Result<Vec<String>> {
    let mut archive = ZipArchive::new(File::open(path)?).map_err(malformed)?;
    let mut names = Vec::new();
    for i in 0..archive.len() {
        names.push(
            archive
                .by_index(i)
                .map_err(malformed)?
                .name()
                .map_err(malformed)?
                .into_owned(),
        );
    }
    Ok(names)
}

fn refusal(result: Result<EditorFile>) -> String {
    match result {
        Ok(_) => "accepted".into(),
        Err(error) => error.to_string(),
    }
}

/// One entry of [`raw_zip`].
type RawEntry<'a> = (&'a str, &'a [u8], CompressionMethod);

const D: CompressionMethod = CompressionMethod::Deflated;
const S: CompressionMethod = CompressionMethod::Stored;

#[test]
fn a_project_with_assets_above_the_old_file_bound_round_trips() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("big.sparksmith");
    // 20 MiB of incompressible bytes: more than the whole former 16 MiB
    // project file and more than the former 8 MiB per asset.
    let large = packed(noise(20 * 1024 * 1024, 1), "model/step");
    let small = packed(b"solid body".to_vec(), "model/gltf-binary");
    let file = with_assets(vec![large.clone(), small.clone()])?;
    write_container(&path, &file)?;
    assert_eq!(read_container(&path)?, file);

    let mut names = entry_names(&path)?;
    assert_eq!(names.remove(0), CONTAINER_DOCUMENT);
    let mut expected = vec![asset_name(large.hash), asset_name(small.hash)];
    expected.sort();
    assert_eq!(names, expected);
    // The document carries hashes and media types, never the bytes.
    let mut archive = ZipArchive::new(File::open(&path)?).map_err(malformed)?;
    let mut text = String::new();
    archive
        .by_name(CONTAINER_DOCUMENT)
        .map_err(malformed)?
        .read_to_string(&mut text)?;
    assert!(text.contains(&large.hash.to_string()));
    assert!(text.contains("\"data\": \"\""));
    assert!(text.len() < 1024 * 1024, "{}", text.len());
    Ok(())
}

#[test]
fn writes_are_reproducible_and_a_project_without_assets_is_one_entry() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (a, b) = (
        dir.path().join("a.sparksmith"),
        dir.path().join("b.sparksmith"),
    );
    let file = with_assets(vec![packed(noise(4096, 2), "model/step")])?;
    write_container(&a, &file)?;
    write_container(&b, &file)?;
    assert_eq!(std::fs::read(&a)?, std::fs::read(&b)?);
    let plain = Engine::demo()?.to_file();
    write_container(&a, &plain)?;
    assert_eq!(entry_names(&a)?, vec![CONTAINER_DOCUMENT.to_string()]);
    assert_eq!(read_container(&a)?, plain);
    Ok(())
}

#[test]
fn missing_mismatched_unreferenced_and_unknown_entries_are_refused_by_name() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("refused.sparksmith");
    let asset = packed(noise(2048, 3), "model/step");
    let other = packed(noise(2048, 4), "model/step");
    let file = with_assets(vec![asset.clone()])?;
    let doc = document(&file)?;
    let name = asset_name(asset.hash);
    let other_name = asset_name(other.hash);
    let mut corrupt = asset.data.to_vec();
    corrupt[0] ^= 0xFF;
    let mut inline = serde_json::to_value(&file)?;
    let mut newer = serde_json::to_value(Engine::demo()?.to_file())?;
    newer["version"] = serde_json::json!(999);
    let newer = serde_json::to_vec(&newer)?;
    let inline_doc = {
        inline["version"] = serde_json::json!(format::FILE_VERSION);
        serde_json::to_vec(&inline)?
    };
    let cases: Vec<(Vec<RawEntry>, String)> = vec![
        (
            vec![(CONTAINER_DOCUMENT, &doc, D)],
            format!("asset {} is missing", asset.hash),
        ),
        (
            vec![(CONTAINER_DOCUMENT, &doc, D), (&name, &corrupt, S)],
            format!("{name:?} does not match its content hash"),
        ),
        (
            vec![
                (CONTAINER_DOCUMENT, &doc, D),
                (&name, &asset.data[..], S),
                (&other_name, &other.data[..], S),
            ],
            format!("{other_name:?} is not an asset the project packs"),
        ),
        (
            vec![
                (CONTAINER_DOCUMENT, &doc, D),
                (&name, &asset.data[..], D),
                ("notes.txt", b"x", D),
            ],
            "\"notes.txt\" is not part of the project format".into(),
        ),
        (
            vec![
                (CONTAINER_DOCUMENT, &doc, D),
                (&name, &asset.data[..], D),
                (CONTAINER_RECOVERY, b"{}", D),
            ],
            "\"recovery.json\" is not part of the project format".into(),
        ),
        (
            vec![(&name, &asset.data[..], D)],
            "has no project.json".into(),
        ),
        (
            vec![
                (CONTAINER_DOCUMENT, &inline_doc, D),
                (&name, &asset.data[..], D),
            ],
            format!("carries the bytes of asset {} inline", asset.hash),
        ),
        (
            vec![(CONTAINER_DOCUMENT, &newer, D)],
            "format version 999, from a newer".into(),
        ),
        (vec![(CONTAINER_DOCUMENT, b"not json", D)], "".into()),
    ];
    for (entries, expected) in cases {
        raw_zip(&path, &entries)?;
        let message = refusal(read_container(&path));
        assert_ne!(message, "accepted", "{expected}");
        assert!(message.contains(&expected), "{message} lacks {expected}");
    }

    // A directory entry.
    let mut zip = ZipWriter::new(File::create(&path)?);
    zip.add_directory("assets/", options(S))
        .map_err(malformed)?;
    zip.start_file(CONTAINER_DOCUMENT, options(D))
        .map_err(malformed)?;
    zip.write_all(&document(&Engine::demo()?.to_file())?)?;
    zip.finish().map_err(malformed)?;
    assert!(refusal(read_container(&path)).contains("is a directory or a link"));

    // A pre-container JSON project is not read as one.
    std::fs::write(&path, format::encode(&file)?)?;
    assert!(refusal(read_container(&path)).contains("container.format"));
    std::fs::write(&path, b"PK")?;
    assert!(refusal(read_container(&path)).contains("container.format"));
    Ok(())
}

#[test]
fn duplicate_entry_names_are_refused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("dup.sparksmith");
    let a = packed(noise(512, 5), "model/step");
    let b = packed(noise(512, 6), "model/step");
    let file = with_assets(vec![a.clone()])?;
    let (na, nb) = (asset_name(a.hash), asset_name(b.hash));
    raw_zip(
        &path,
        &[
            (CONTAINER_DOCUMENT, &document(&file)?, D),
            (&na, &a.data[..], S),
            (&nb, &a.data[..], S),
        ],
    )?;
    // Rename the second entry to the first in its local and central headers.
    let mut bytes = std::fs::read(&path)?;
    let (from, to) = (nb.as_bytes(), na.as_bytes());
    let mut replaced = 0;
    let mut i = 0;
    while i + from.len() <= bytes.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + from.len()].copy_from_slice(to);
            replaced += 1;
        }
        i += 1;
    }
    assert_eq!(replaced, 2);
    std::fs::write(&path, bytes)?;
    let message = refusal(read_container(&path));
    assert!(
        message.contains("names one entry twice") || message.contains("duplicate"),
        "{message}"
    );
    Ok(())
}

#[test]
fn bounds_are_measured_on_decompressed_bytes_and_entry_counts() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("bounds.sparksmith");
    // A deflate bomb: 16 MiB + 1 of spaces as the document.
    let bomb = vec![b' '; MAX_FILE_BYTES + 1];
    raw_zip(&path, &[(CONTAINER_DOCUMENT, &bomb, D)])?;
    assert!(std::fs::metadata(&path)?.len() < 1024 * 1024);
    assert!(matches!(read_container(&path), Err(Error::TooLarge(_))));

    // More entries than a container holds.
    let names: Vec<String> = (0..=CONTAINER_MAX_ENTRIES)
        .map(|i| asset_name(asset_hash(&i.to_le_bytes())))
        .collect();
    let entries: Vec<(&str, &[u8], CompressionMethod)> =
        names.iter().map(|n| (n.as_str(), &b""[..], S)).collect();
    raw_zip(&path, &entries)?;
    assert!(matches!(read_container(&path), Err(Error::TooLarge(_))));

    // A file larger than a container may be.
    let large = File::create(&path)?;
    large.set_len(CONTAINER_MAX_BYTES + 1)?;
    assert!(matches!(read_container(&path), Err(Error::TooLarge(_))));
    Ok(())
}

#[test]
fn a_corrupted_entry_fails_its_checksum_and_a_failed_write_keeps_the_target() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("crc.sparksmith");
    let asset = packed(noise(4096, 7), "model/step");
    let file = with_assets(vec![asset.clone()])?;
    raw_zip(
        &path,
        &[
            (CONTAINER_DOCUMENT, &document(&file)?, D),
            (&asset_name(asset.hash), &asset.data[..], S),
        ],
    )?;
    assert_eq!(read_container(&path)?, file);
    let mut bytes = std::fs::read(&path)?;
    let at = bytes
        .windows(16)
        .position(|w| w == &asset.data[..16])
        .ok_or_else(|| Error::Invalid("stored asset bytes".into()))?;
    bytes[at + 100] ^= 0x55;
    std::fs::write(&path, &bytes)?;
    assert!(read_container(&path).is_err());

    // A mismatched asset is refused before the target is touched.
    let target = dir.path().join("kept.sparksmith");
    write_container(&target, &file)?;
    let original = std::fs::read(&target)?;
    let mut bad = file.clone();
    bad.project.library.assets[0].data.make_mut()[0] ^= 1;
    assert!(write_container(&target, &bad).is_err());
    assert_eq!(std::fs::read(&target)?, original);
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 2);
    Ok(())
}

#[test]
fn one_asset_at_the_raised_limit_round_trips_and_one_byte_more_is_refused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("limit.sparksmith");
    let limit = sparksmith_core::library::bundle::MAX_ASSET_BYTES;
    let file = with_assets(vec![packed(noise(limit, 8), "model/step")])?;
    write_container(&path, &file)?;
    assert_eq!(read_container(&path)?, file);
    let over = packed(noise(limit + 1, 9), "model/step");
    assert!(with_assets(vec![over.clone()]).is_err());
    let mut forced = Engine::demo()?.to_file();
    forced.project.library.assets = vec![over].into();
    assert!(write_container(&dir.path().join("over.sparksmith"), &forced).is_err());
    assert!(!dir.path().join("over.sparksmith").exists());
    Ok(())
}
