//! Private distribution of the MIT-licensed resurrect plugin, shipped in the binary.
use std::path::{Path, PathBuf};

use crate::api::schema::InstalledPluginInfo;

include!("bundled_resurrect_assets.rs");
const PLUGIN_ID: &str = "ntindle.herdr-resurrect";

fn asset_directory(base: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for (name, bytes) in ASSETS {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(bytes);
    }
    base.join(format!("resurrect-{:x}", digest.finalize()))
}

// Called under the registry lock. Content-addressed directories let old servers
// keep running their own bundled version during an update or live handoff.
fn install_at(base: &Path) -> std::io::Result<PathBuf> {
    let root = asset_directory(base);
    for (name, bytes) in ASSETS {
        let destination = root.join(name);
        if std::fs::read(&destination).is_ok_and(|current| current == *bytes) {
            continue;
        }
        let parent = destination
            .parent()
            .ok_or_else(|| std::io::Error::other("invalid bundled asset path"))?;
        std::fs::create_dir_all(parent)?;
        let temporary = destination.with_extension(format!("bundle-{}.tmp", std::process::id()));
        std::fs::write(&temporary, bytes)?;
        crate::platform::replace_file(&temporary, &destination)?;
    }
    Ok(root)
}

fn register(entries: &mut Vec<InstalledPluginInfo>, base: &Path) -> std::io::Result<()> {
    let existing = entries
        .iter()
        .position(|entry| entry.plugin_id == PLUGIN_ID);
    if existing.is_some_and(|i| !Path::new(&entries[i].plugin_root).starts_with(base)) {
        // Explicit local or GitHub installations remain user-controlled.
        return Ok(());
    }
    let root = install_at(base)?;
    let enabled = existing.is_none_or(|i| entries[i].enabled);
    let plugin = crate::app::load_plugin_manifest(&root.to_string_lossy(), enabled)
        .map_err(|(_, message)| std::io::Error::other(message))?;
    if let Some(i) = existing {
        entries[i] = plugin;
    } else {
        entries.push(plugin);
    }
    Ok(())
}

pub(crate) fn ensure_installed() -> std::io::Result<()> {
    let base = crate::plugin_paths::managed_plugins_dir().join("bundled");
    let (result, _) = crate::persist::plugin_registry::update(|entries| register(entries, &base))?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_resurrect_installs_offline_preserves_disabled_and_custom_installations() {
        let base =
            std::env::temp_dir().join(format!("herdr-bundled-resurrect-{}", std::process::id()));
        let mut entries = Vec::new();
        register(&mut entries, &base).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_id, PLUGIN_ID);
        assert!(entries[0].enabled);
        assert_eq!(entries[0].actions.len(), 7);
        let root = PathBuf::from(&entries[0].plugin_root);
        assert!(root.join("LICENSE").is_file());
        assert!(root.join("lib/restore.js").is_file());
        entries[0].enabled = false;
        register(&mut entries, &base).unwrap();
        assert!(!entries[0].enabled);
        assert_eq!(entries.len(), 1);
        entries[0].plugin_root = "/custom/user/plugin".into();
        register(&mut entries, &base).unwrap();
        assert_eq!(entries[0].plugin_root, "/custom/user/plugin");
        std::fs::remove_dir_all(base).unwrap();
    }
}
