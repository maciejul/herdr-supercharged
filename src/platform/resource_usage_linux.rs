use super::{ProcessUsage, ResourceUsage};
use std::{collections::HashMap, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Claude,
    Codex,
    Mcp,
}

fn classify(name: &str, argv: &[String]) -> Option<Kind> {
    if name == "claude" {
        return Some(Kind::Claude);
    }
    if name == "codex" {
        return Some(Kind::Codex);
    }
    // Restrict matching to executable/script/package slots, never shell code or search text.
    let executable = argv.first().map(|s| {
        Path::new(s)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("")
    });
    let candidate = match executable {
        Some("node" | "nodejs" | "bun" | "deno" | "python" | "python3") => {
            match argv.get(1).map(String::as_str) {
                Some("-e" | "-c" | "--eval") => None,
                Some("-m") => argv.get(2),
                Some(_) => argv.get(1),
                None => None,
            }
        }
        Some("npm" | "npx" | "uvx") => argv
            .iter()
            .skip(1)
            .take(4)
            .find(|arg| !arg.starts_with('-') && !matches!(arg.as_str(), "exec" | "run")),
        _ => argv.first(),
    };
    if candidate.is_some_and(|arg| {
        let file = Path::new(arg)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("");
        file.starts_with("mcp-server")
            || file.starts_with("chrome-devtools-mcp")
            || file.ends_with("-mcp")
            || file.contains("-mcp@")
            || arg.contains("/context-mode/")
            || arg.contains("/mcp-server/")
            || arg.contains("/@modelcontextprotocol/")
            || arg.contains("/mcp/dist/")
    }) {
        Some(Kind::Mcp)
    } else {
        None
    }
}

fn pss_bytes(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        line.strip_prefix("Pss:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}

struct Process {
    parent: u32,
    kind: Option<Kind>,
    memory: Option<u64>,
}

pub(super) fn sample() -> ResourceUsage {
    let mut result = ResourceUsage::default();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        result.error = Some("Cannot read /proc".into());
        return result;
    };
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let mut processes = HashMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if uid.is_none() || std::fs::metadata(&path).ok().map(|m| m.uid()) != uid {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(path.join("status")) else {
            continue;
        };
        let name = status
            .lines()
            .find_map(|l| l.strip_prefix("Name:"))
            .unwrap_or("")
            .trim();
        let parent = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let argv = std::fs::read(path.join("cmdline"))
            .unwrap_or_default()
            .split(|b| *b == 0)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect::<Vec<_>>();
        let kind = classify(name, &argv);
        processes.insert(
            pid,
            Process {
                parent,
                kind,
                memory: None,
            },
        );
    }
    // Count wrapper children once, even when their executable name is just node.
    let inherited = processes
        .iter()
        .filter_map(|(&pid, process)| {
            if process.kind.is_some() {
                return None;
            }
            let mut parent = process.parent;
            for _ in 0..32 {
                let ancestor = processes.get(&parent)?;
                if ancestor.kind == Some(Kind::Mcp) {
                    return Some(pid);
                }
                if ancestor.kind.is_some() || ancestor.parent == parent {
                    break;
                }
                parent = ancestor.parent;
            }
            None
        })
        .collect::<Vec<_>>();
    for pid in inherited {
        if let Some(p) = processes.get_mut(&pid) {
            p.kind = Some(Kind::Mcp);
        }
    }
    for (pid, process) in &mut processes {
        let usage: &mut ProcessUsage = match process.kind {
            Some(Kind::Claude) => &mut result.claude,
            Some(Kind::Codex) => &mut result.codex,
            Some(Kind::Mcp) => &mut result.mcp,
            None => continue,
        };
        process.memory = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            .ok()
            .and_then(|s| pss_bytes(&s));
        usage.processes += 1;
        if let Some(bytes) = process.memory {
            usage.measured += 1;
            usage.memory_bytes += bytes;
        }
    }
    result.sampled_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_usage_classification_and_pss() {
        assert_eq!(classify("claude", &[]), Some(Kind::Claude));
        assert_eq!(classify("codex", &[]), Some(Kind::Codex));
        assert_eq!(
            classify(
                "node",
                &["/tmp/node_modules/context-mode/dist/index.js".into()]
            ),
            Some(Kind::Mcp)
        );
        assert_eq!(
            classify(
                "node",
                &["/tmp/node_modules/@modelcontextprotocol/server/dist/index.js".into()]
            ),
            Some(Kind::Mcp)
        );
        assert_eq!(classify("sh", &["please debug mcp memory".into()]), None);
        assert_eq!(
            pss_bytes("Rss: 900 kB\nPss: 123 kB\nPss_Dirty: 12 kB"),
            Some(123 * 1024)
        );
        assert_eq!(
            classify("rg", &["rg".into(), "/context-mode/".into(), "src".into()]),
            None
        );
        assert_eq!(
            classify(
                "sh",
                &["sh".into(), "-c".into(), "cat /context-mode/file".into()]
            ),
            None
        );
        assert_eq!(pss_bytes("Rss: 900 kB"), None);
    }
}
