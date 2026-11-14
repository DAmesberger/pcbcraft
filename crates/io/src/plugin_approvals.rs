//! Plugin approvals (extension-ABI slice S8): which plugin bundles this
//! user has approved, by bundle hash. Per user, in the config directory
//! (`plugin-approvals.json`), never in a project: a project holds the
//! modules its parts were made by, never a grant.
//!
//! A bundle is approved by its hash (manifest, module and part files), so
//! any changed file asks again. What a person sees when it asks again is
//! [`Standing::Changed`]: the approval of the same plugin id under another
//! bundle and, in plain words, what changed (version, capabilities and
//! their effects, needs, licence, or only the files). The store records
//! what was approved (id, version, title, licence, capabilities with their
//! effects, needs), so that comparison needs no old bundle.
//!
//! Who reads the store: GP9 (no unapproved third-party bundle runs on
//! open), and the `read` and `edit` capabilities when they are served
//! (S9). Effect-`none` capabilities need no grant: a pure plugin's `needs`
//! are empty.
use crate::library_folder::{pretty, read_bounded};
use crate::persistence::atomic_write;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sparksmith_core::ids::ContentHash;
use sparksmith_generators::abi::v1::{Description, Effect, Need};
use std::path::Path;

/// The approvals file's format; any other is refused.
pub const APPROVALS_FORMAT: u32 = 1;
pub const APPROVALS_FILE: &str = "plugin-approvals.json";
/// Approvals one user keeps.
pub const MAX_APPROVALS: usize = 1024;
/// Bytes of the approvals file.
pub const MAX_APPROVALS_BYTES: usize = 4 * 1024 * 1024;

/// One capability as it was approved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedCapability {
    pub name: String,
    pub effect: Effect,
}

/// One approved bundle and what it declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub bundle: ContentHash,
    pub id: String,
    pub version: u32,
    pub title: String,
    pub licence: String,
    pub capabilities: Vec<ApprovedCapability>,
    pub needs: Vec<Need>,
}

impl Approval {
    /// The approval of `bundle`, whose plugin describes itself as
    /// `description`.
    pub fn of(bundle: ContentHash, description: &Description) -> Self {
        Self {
            bundle,
            id: description.id.clone(),
            version: description.version,
            title: description.title.clone(),
            licence: description.licence.clone(),
            capabilities: description
                .capabilities
                .iter()
                .map(|c| ApprovedCapability {
                    name: c.name.clone(),
                    effect: c.effect,
                })
                .collect(),
            needs: description.needs.clone(),
        }
    }
}

/// The user's approvals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approvals {
    pub format: u32,
    pub approved: Vec<Approval>,
}

impl Default for Approvals {
    fn default() -> Self {
        Self {
            format: APPROVALS_FORMAT,
            approved: Vec::new(),
        }
    }
}

/// Where a bundle stands with this user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// This very bundle is approved.
    Approved,
    /// The same plugin was approved as another bundle: it asks again,
    /// showing what changed.
    Changed {
        previous: Box<Approval>,
        changes: Vec<String>,
    },
    /// Never approved.
    New,
}

fn effect_word(effect: Effect) -> &'static str {
    match effect {
        Effect::None => "none",
        Effect::Read => "read",
        Effect::Edit => "edit",
    }
}

fn need_text(need: &Need) -> String {
    format!(
        "{} {}.{} ({})",
        need.module,
        need.major,
        need.minor,
        effect_word(need.class)
    )
}

/// What changed from `before` to `after`, in plain words; the files alone
/// when nothing the plugin declares changed.
pub fn changes(before: &Approval, after: &Approval) -> Vec<String> {
    let mut out = Vec::new();
    if before.version != after.version {
        out.push(format!("version {} → {}", before.version, after.version));
    }
    if before.title != after.title {
        out.push(format!("title ‘{}’ → ‘{}’", before.title, after.title));
    }
    if before.licence != after.licence {
        out.push(format!("licence {} → {}", before.licence, after.licence));
    }
    for c in &after.capabilities {
        match before.capabilities.iter().find(|b| b.name == c.name) {
            None => out.push(format!(
                "new capability {} (effect {})",
                c.name,
                effect_word(c.effect)
            )),
            Some(b) if b.effect != c.effect => out.push(format!(
                "capability {}: effect {} → {}",
                c.name,
                effect_word(b.effect),
                effect_word(c.effect)
            )),
            Some(_) => {}
        }
    }
    for b in &before.capabilities {
        if !after.capabilities.iter().any(|c| c.name == b.name) {
            out.push(format!("capability {} removed", b.name));
        }
    }
    for n in &after.needs {
        if !before.needs.contains(n) {
            out.push(format!("now needs {}", need_text(n)));
        }
    }
    for n in &before.needs {
        if !after.needs.contains(n) {
            out.push(format!("no longer needs {}", need_text(n)));
        }
    }
    if out.is_empty() {
        out.push("the same declarations, with changed files".into());
    }
    out
}

impl Approvals {
    /// Bounded, each bundle once, each plugin id once.
    pub fn check(&self) -> Result<()> {
        if self.format != APPROVALS_FORMAT {
            return Err(Error::Invalid(format!(
                "{APPROVALS_FILE} is format {}; this program reads format {APPROVALS_FORMAT}",
                self.format
            )));
        }
        if self.approved.len() > MAX_APPROVALS {
            return Err(Error::Invalid(format!(
                "{APPROVALS_FILE} keeps at most {MAX_APPROVALS} approvals"
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut bundles = std::collections::BTreeSet::new();
        for a in &self.approved {
            if !ids.insert(a.id.as_str()) || !bundles.insert(a.bundle) {
                return Err(Error::Invalid(format!(
                    "{APPROVALS_FILE} approves the plugin {} or its bundle twice",
                    a.id
                )));
            }
        }
        Ok(())
    }

    /// Where the bundle `bundle` of the plugin `description` stands.
    pub fn standing(&self, bundle: ContentHash, description: &Description) -> Standing {
        if self.approved.iter().any(|a| a.bundle == bundle) {
            return Standing::Approved;
        }
        match self.approved.iter().find(|a| a.id == description.id) {
            Some(previous) => Standing::Changed {
                changes: changes(previous, &Approval::of(bundle, description)),
                previous: Box::new(previous.clone()),
            },
            None => Standing::New,
        }
    }

    /// Approve `bundle`, replacing any approval of the same plugin id.
    pub fn approve(&mut self, bundle: ContentHash, description: &Description) -> Result<()> {
        self.approved
            .retain(|a| a.id != description.id && a.bundle != bundle);
        if self.approved.len() >= MAX_APPROVALS {
            return Err(Error::Invalid(format!(
                "at most {MAX_APPROVALS} plugins can be approved; revoke one first"
            )));
        }
        self.approved.push(Approval::of(bundle, description));
        self.approved.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(())
    }

    /// Withdraw the approval of `bundle`; whether there was one.
    pub fn revoke(&mut self, bundle: ContentHash) -> bool {
        let before = self.approved.len();
        self.approved.retain(|a| a.bundle != bundle);
        self.approved.len() != before
    }
}

/// Read the approvals; a file that is not there is none. A file that
/// cannot be read or checked is refused, never taken for "none".
pub fn read_approvals(path: &Path) -> Result<Approvals> {
    if !path.try_exists()? {
        return Ok(Approvals::default());
    }
    let approvals: Approvals = serde_json::from_slice(&read_bounded(path, MAX_APPROVALS_BYTES)?)?;
    approvals.check()?;
    Ok(approvals)
}

/// Check and atomically replace the approvals file.
pub fn write_approvals(path: &Path, approvals: &Approvals) -> Result<()> {
    approvals.check()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic_write(path, &pretty(approvals)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sparksmith_generators::abi::v1::{ABI, Capability, GENERATE_PART};

    fn plugin(version: u32, edit: bool) -> Description {
        let capability = |name: &str, effect| Capability {
            name: name.into(),
            effect,
            title: None,
            params: Vec::new(),
            rules: Vec::new(),
            presets: Vec::new(),
            part: None,
        };
        let mut capabilities = vec![capability(GENERATE_PART, Effect::None)];
        let mut needs = Vec::new();
        if edit {
            capabilities.push(capability("stitch", Effect::Edit));
            needs.push(Need {
                module: "layout".into(),
                major: 1,
                minor: 2,
                class: Effect::Edit,
            });
        }
        Description {
            abi: ABI,
            id: "example.stitch".into(),
            version,
            title: "Via stitching".into(),
            licence: "MIT".into(),
            deterministic: true,
            capabilities,
            needs,
        }
    }

    #[test]
    fn a_changed_bundle_asks_again_and_says_what_changed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(APPROVALS_FILE);
        let mut approvals = read_approvals(&path)?;
        let first = ContentHash([1; 32]);
        assert_eq!(approvals.standing(first, &plugin(1, false)), Standing::New);
        approvals.approve(first, &plugin(1, false))?;
        write_approvals(&path, &approvals)?;
        let approvals = read_approvals(&path)?;
        assert_eq!(
            approvals.standing(first, &plugin(1, false)),
            Standing::Approved
        );
        // Other files, the same declarations.
        let rebuilt = ContentHash([2; 32]);
        let Standing::Changed { changes, previous } =
            approvals.standing(rebuilt, &plugin(1, false))
        else {
            return Err(Error::Invalid(
                "a changed bundle was not asked again".into(),
            ));
        };
        assert_eq!(previous.bundle, first);
        assert_eq!(changes, ["the same declarations, with changed files"]);
        // A newer version that adds an edit command and needs layout.
        let Standing::Changed { changes, .. } = approvals.standing(rebuilt, &plugin(2, true))
        else {
            return Err(Error::Invalid("not asked again".into()));
        };
        assert_eq!(
            changes,
            [
                "version 1 → 2",
                "new capability stitch (effect edit)",
                "now needs layout 1.2 (edit)"
            ]
        );
        Ok(())
    }

    #[test]
    fn approving_replaces_the_plugins_old_bundle_and_revoking_removes_it() -> Result<()> {
        let mut approvals = Approvals::default();
        approvals.approve(ContentHash([1; 32]), &plugin(1, false))?;
        approvals.approve(ContentHash([2; 32]), &plugin(2, false))?;
        assert_eq!(approvals.approved.len(), 1);
        assert_eq!(approvals.approved[0].bundle, ContentHash([2; 32]));
        assert!(!approvals.revoke(ContentHash([1; 32])));
        assert!(approvals.revoke(ContentHash([2; 32])));
        assert!(approvals.approved.is_empty());
        // A file approving one plugin twice is refused.
        let mut twice = Approvals::default();
        twice
            .approved
            .push(Approval::of(ContentHash([1; 32]), &plugin(1, false)));
        twice
            .approved
            .push(Approval::of(ContentHash([2; 32]), &plugin(1, false)));
        assert!(twice.check().is_err());
        Ok(())
    }
}
