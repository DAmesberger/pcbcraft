//! A project or library tree in memory: a sorted map from path to bytes
//! (docs/file-format.md, "The tree"). The tree encoder produces one and
//! `sparksmith-io` puts it in a folder or a ZIP; nothing here touches a
//! file. Asset bytes are shared with the document, never copied.
use crate::diagnostics::{Error, Result};
use crate::library::bundle::AssetBytes;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Characters of one path, slashes included.
pub const MAX_PATH_BYTES: usize = 255;
/// Characters of one file or folder name.
pub const MAX_NAME_BYTES: usize = 64;
/// Folder levels of one path, the file name included.
pub const MAX_PATH_DEPTH: usize = 8;
/// Files one tree holds (target).
pub const MAX_TREE_FILES: usize = 65_536;
/// Bytes of one tree, every file counted once (target): the container's
/// limit, so a tree always fits a ZIP.
pub const MAX_TREE_BYTES: u64 = super::CONTAINER_MAX_BYTES;
/// Bytes of one collection file, `.jsonl` (target).
pub const MAX_COLLECTION_BYTES: usize = 64 * 1024 * 1024;
/// Bytes of one document file, `.json`: a library folder's revision limit.
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
/// Bytes of one line of a collection file (target).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Where assets live, by the SHA-256 of their bytes.
pub const ASSET_FOLDER: &str = "assets/sha256/";

/// The bytes of one file: text the encoder wrote, or an asset shared with
/// the document's library.
#[derive(Clone)]
pub enum FileBytes {
    Text(Arc<[u8]>),
    Asset(AssetBytes),
}

impl FileBytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Text(bytes) => bytes,
            Self::Asset(asset) => asset.as_slice(),
        }
    }

    fn sha256_hex(&self) -> String {
        match self {
            Self::Text(bytes) => {
                sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(bytes))
            }
            Self::Asset(asset) => sparksmith_generator_abi::hex(&asset.hash().0),
        }
    }
}

impl PartialEq for FileBytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl std::fmt::Debug for FileBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} bytes", self.as_slice().len())
    }
}

/// What a path holds, from its place and extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// Layout A, `.jsonl`.
    Collection,
    /// Layout B, `.json`.
    Document,
    /// `assets/sha256/<hash>`.
    Asset,
}

impl FileKind {
    fn limit(self) -> u64 {
        match self {
            Self::Collection => MAX_COLLECTION_BYTES as u64,
            Self::Document => MAX_DOCUMENT_BYTES as u64,
            Self::Asset => super::CONTAINER_MAX_ASSET_BYTES,
        }
    }
}

/// Check `path` against the tree's path rules and say what it holds:
/// lower-case ASCII names `[a-z0-9._-]` of at most [`MAX_NAME_BYTES`],
/// separated by `/`, no name starting with a dot, at most
/// [`MAX_PATH_DEPTH`] levels; a `.jsonl` collection, a `.json` document or
/// an asset named by its 64-digit hash.
pub fn file_kind(path: &str) -> Result<FileKind> {
    let refuse = |why: &str| {
        Err(Error::Invalid(format!(
            "the path \"{}\" {why}",
            shown(path)
        )))
    };
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return refuse(&format!("must have 1 to {MAX_PATH_BYTES} characters"));
    }
    let names: Vec<&str> = path.split('/').collect();
    if names.len() > MAX_PATH_DEPTH {
        return refuse(&format!("is more than {MAX_PATH_DEPTH} levels deep"));
    }
    for name in &names {
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return refuse(&format!(
                "has a name that is empty or longer than {MAX_NAME_BYTES} characters"
            ));
        }
        if name.starts_with('.') {
            return refuse("has a name starting with a dot");
        }
        if !name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        }) {
            return refuse(
                "may use only lower-case letters, digits, '.', '_' and '-' in its names",
            );
        }
    }
    if let Some(hash) = path.strip_prefix(ASSET_FOLDER) {
        if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(FileKind::Asset);
        }
        return refuse("is in the asset folder but is not named by a 64-digit SHA-256");
    }
    if path.ends_with(".jsonl") {
        Ok(FileKind::Collection)
    } else if path.ends_with(".json") {
        Ok(FileKind::Document)
    } else {
        refuse("is neither a .jsonl collection, a .json document nor an asset")
    }
}

/// A path for a message, cut short and without control characters.
fn shown(path: &str) -> String {
    path.chars()
        .take(MAX_PATH_BYTES)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// A tree: every file by path, sorted by the path's bytes. Bounded by
/// [`MAX_TREE_FILES`] and [`MAX_TREE_BYTES`], each file by its kind's
/// limit; an asset's name is checked against its bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSet {
    files: BTreeMap<String, FileBytes>,
    bytes: u64,
}

impl FileSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace one file, refusing a path the rules do not allow,
    /// bytes over the file's or the tree's limit, and an asset whose bytes
    /// do not hash to its name. A refused file leaves the set unchanged.
    pub fn insert(&mut self, path: &str, bytes: FileBytes) -> Result<()> {
        let kind = file_kind(path)?;
        let size = bytes.as_slice().len() as u64;
        if size > kind.limit() {
            return Err(Error::Limit(format!(
                "{path} is {size} bytes, more than the {} allowed for it",
                kind.limit()
            )));
        }
        if kind == FileKind::Asset {
            let named = path.strip_prefix(ASSET_FOLDER).unwrap_or(path);
            if bytes.sha256_hex() != named {
                return Err(Error::Invalid(format!(
                    "{path}: the bytes do not hash to the file's name"
                )));
            }
        }
        let replaced = self
            .files
            .get(path)
            .map_or(0, |old| old.as_slice().len() as u64);
        if replaced == 0 && !self.files.contains_key(path) && self.files.len() >= MAX_TREE_FILES {
            return Err(Error::Limit(format!(
                "the tree would hold more than {MAX_TREE_FILES} files"
            )));
        }
        let total = self.bytes - replaced + size;
        if total > MAX_TREE_BYTES {
            return Err(Error::Limit(format!(
                "the tree would hold more than {MAX_TREE_BYTES} bytes"
            )));
        }
        self.files.insert(path.to_owned(), bytes);
        self.bytes = total;
        Ok(())
    }

    /// Add text the encoder wrote.
    pub fn insert_text(&mut self, path: &str, bytes: Vec<u8>) -> Result<()> {
        self.insert(path, FileBytes::Text(bytes.into()))
    }

    /// Add an asset at its content path, sharing its bytes.
    pub fn insert_asset(&mut self, asset: &AssetBytes) -> Result<String> {
        let path = format!(
            "{ASSET_FOLDER}{}",
            sparksmith_generator_abi::hex(&asset.hash().0)
        );
        self.insert(&path, FileBytes::Asset(asset.clone()))?;
        Ok(path)
    }

    pub fn get(&self, path: &str) -> Option<&FileBytes> {
        self.files.get(path)
    }

    pub fn remove(&mut self, path: &str) -> Option<FileBytes> {
        let removed = self.files.remove(path)?;
        self.bytes -= removed.as_slice().len() as u64;
        Some(removed)
    }

    /// Every file, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &FileBytes)> {
        self.files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Bytes of every file together.
    pub fn total_bytes(&self) -> u64 {
        self.bytes
    }

    /// The tree digest (docs/file-format.md, "Hashes"): the SHA-256, as 64
    /// lower-case hex digits, of one line `<sha256 of the file> <path>\n`
    /// per file in path order, the hash in lower-case hex and one space
    /// between. Computed, never stored in the tree.
    pub fn digest(&self) -> String {
        let mut listing = Vec::new();
        for (path, bytes) in &self.files {
            listing.extend_from_slice(bytes.sha256_hex().as_bytes());
            listing.push(b' ');
            listing.extend_from_slice(path.as_bytes());
            listing.push(b'\n');
        }
        sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(&listing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Fallible = std::result::Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn paths_follow_the_rules() {
        assert_eq!(
            file_kind("board/tracks.jsonl").ok(),
            Some(FileKind::Collection)
        );
        assert_eq!(
            file_kind("sparksmith-project.json").ok(),
            Some(FileKind::Document)
        );
        let hash = "a".repeat(64);
        assert_eq!(
            file_kind(&format!("assets/sha256/{hash}")).ok(),
            Some(FileKind::Asset)
        );
        for bad in [
            "",
            "Board/tracks.jsonl",
            "board//tracks.jsonl",
            "/board/tracks.jsonl",
            "board/../tracks.jsonl",
            "board/.hidden.json",
            "board/tracks.txt",
            "board/tracks",
            "board/sp ace.json",
            "assets/sha256/abc",
            "a/b/c/d/e/f/g/h/i.json",
        ] {
            assert!(file_kind(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(file_kind(&format!("{}.json", "a".repeat(60))).is_err());
    }

    #[test]
    fn files_are_sorted_and_bounded() -> Fallible {
        let mut set = FileSet::new();
        set.insert_text("board/tracks.jsonl", b"b\n".to_vec())?;
        set.insert_text("board/board.json", b"a\n".to_vec())?;
        set.insert_text("circuit/nets.jsonl", b"c\n".to_vec())?;
        let paths: Vec<&str> = set.iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            [
                "board/board.json",
                "board/tracks.jsonl",
                "circuit/nets.jsonl"
            ]
        );
        assert_eq!(set.total_bytes(), 6);
        set.insert_text("board/board.json", b"aaaa\n".to_vec())?;
        assert_eq!(set.total_bytes(), 9);
        assert!(set.remove("board/board.json").is_some());
        assert_eq!(set.total_bytes(), 4);
        let refused = set.insert_text("board/board.json", vec![b' '; MAX_DOCUMENT_BYTES + 1]);
        assert!(matches!(refused, Err(Error::Limit(_))), "{refused:?}");
        assert!(set.get("board/board.json").is_none());
        Ok(())
    }

    #[test]
    fn assets_are_shared_and_checked_against_their_name() -> Fallible {
        let asset = AssetBytes::new(b"model bytes".to_vec());
        let mut set = FileSet::new();
        let path = set.insert_asset(&asset)?;
        assert!(path.starts_with(ASSET_FOLDER));
        match set.get(&path) {
            Some(FileBytes::Asset(shared)) => assert!(shared.shares(&asset)),
            other => return Err(format!("not shared: {other:?}").into()),
        }
        let wrong = format!("{ASSET_FOLDER}{}", "0".repeat(64));
        assert!(
            set.insert(&wrong, FileBytes::Text(b"model bytes".to_vec().into()))
                .is_err()
        );
        Ok(())
    }

    /// The digest is the hash of a `sha256sum`-style listing in path order,
    /// so it changes with any byte or name and not with insertion order.
    #[test]
    fn the_digest_covers_every_name_and_byte() -> Fallible {
        let mut a = FileSet::new();
        a.insert_text("properties.json", b"{}\n".to_vec())?;
        a.insert_text("board/tracks.jsonl", b"x\n".to_vec())?;
        let mut b = FileSet::new();
        b.insert_text("board/tracks.jsonl", b"x\n".to_vec())?;
        b.insert_text("properties.json", b"{}\n".to_vec())?;
        assert_eq!(a.digest(), b.digest());
        let listing = format!(
            "{} board/tracks.jsonl\n{} properties.json\n",
            sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(b"x\n")),
            sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(b"{}\n")),
        );
        assert_eq!(
            a.digest(),
            sparksmith_generator_abi::hex(&sparksmith_generator_abi::sha256(listing.as_bytes()))
        );
        b.insert_text("board/tracks.jsonl", b"y\n".to_vec())?;
        assert_ne!(a.digest(), b.digest());
        Ok(())
    }
}
