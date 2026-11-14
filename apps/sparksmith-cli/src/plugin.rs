//! `plugin approvals | approve NAME | revoke SHA256`: the user's plugin
//! approvals (extension-ABI slice S8), by bundle hash, kept in the user's
//! config directory and never in a project. `approve` takes a shipped
//! generator bundle by name, describes its module and says what changed
//! when the plugin was approved before as another bundle.
use super::Result;
use sparksmith_core::ids::ContentHash;
use sparksmith_core::product::SHORT_NAME as CLI;
use sparksmith_generators::abi::GeneratorRuntime;
use sparksmith_io::plugin_approvals::{Standing, read_approvals, write_approvals};
use sparksmith_io::settings::UserDirs;
use sparksmith_io::shipped::ShippedLibrary;

pub const USAGE: &str =
    "plugin approvals                 the plugin bundles you approved, by bundle hash
  plugin approve NAME              approve the shipped generator bundle NAME (shows what changed)
  plugin revoke SHA256             withdraw the approval of a bundle";

pub fn run(args: &[String]) -> Result<()> {
    let usage = || format!("usage: {CLI} {USAGE}");
    let path = UserDirs::from_env()?.approvals_path();
    match args {
        [command] if command == "approvals" => {
            let approvals = read_approvals(&path)?;
            if approvals.approved.is_empty() {
                println!("No plugin is approved ({}).", path.display());
            }
            for a in &approvals.approved {
                let effects: Vec<String> = a
                    .capabilities
                    .iter()
                    .map(|c| format!("{} ({:?})", c.name, c.effect).to_lowercase())
                    .collect();
                println!(
                    "{} {} version {}: {} [{}]",
                    a.bundle,
                    a.id,
                    a.version,
                    a.title,
                    effects.join(", ")
                );
            }
            Ok(())
        }
        [command, name] if command == "approve" => {
            let bundle = ShippedLibrary::locate()?.generator(name)?;
            let description = sparksmith_generators::worker::Worker::default()
                .describe(&bundle.module)
                .map_err(|f| f.to_string())?;
            let mut approvals = read_approvals(&path)?;
            match approvals.standing(bundle.hash, &description) {
                Standing::Approved => {
                    println!(
                        "{} ({}) is already approved.",
                        description.title, bundle.hash
                    );
                    return Ok(());
                }
                Standing::Changed { previous, changes } => println!(
                    "{} was approved as bundle {}; this bundle {} changes: {}.",
                    description.title,
                    previous.bundle,
                    bundle.hash,
                    changes.join("; ")
                ),
                Standing::New => {}
            }
            approvals.approve(bundle.hash, &description)?;
            write_approvals(&path, &approvals)?;
            println!(
                "Approved {} version {} (bundle {}).",
                description.title, description.version, bundle.hash
            );
            Ok(())
        }
        [command, hash] if command == "revoke" => {
            let hash = ContentHash::parse(hash)
                .ok_or_else(|| format!("{hash} is not a SHA-256 in hex"))?;
            let mut approvals = read_approvals(&path)?;
            if approvals.revoke(hash) {
                write_approvals(&path, &approvals)?;
                println!("Revoked the approval of bundle {hash}.");
            } else {
                println!("Bundle {hash} was not approved.");
            }
            Ok(())
        }
        _ => Err(usage().into()),
    }
}
