use super::history::HistoryEntry;

/// Parse fish shell history (`~/.local/share/fish/fish_history`).
/// Format:
///   - cmd: <command>
///     when: <unix_timestamp>
pub fn parse(data: &[u8]) -> Option<Vec<HistoryEntry>> {
    let text = std::str::from_utf8(data).ok()?;
    let mut entries = Vec::new();
    let mut current_cmd: Option<String> = None;
    let mut current_ts: Option<i64> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(cmd) = line.strip_prefix("- cmd:") {
            if let (Some(cmd), Some(ts)) = (current_cmd.take(), current_ts.take()) {
                entries.push(HistoryEntry {
                    timestamp: ts,
                    command: cmd,
                });
            }
            current_cmd = Some(cmd.trim().to_string());
        } else if let Some(ts_str) = line.strip_prefix("when:") {
            if let Ok(ts) = ts_str.trim().parse::<i64>() {
                current_ts = Some(ts);
            }
        }
    }

    if let (Some(cmd), Some(ts)) = (current_cmd, current_ts) {
        entries.push(HistoryEntry {
            timestamp: ts,
            command: cmd,
        });
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

pub fn serialize(entries: &[HistoryEntry]) -> Vec<u8> {
    let mut out = String::new();
    for e in entries {
        out.push_str(&format!("- cmd: {}\n  when: {}\n", e.command, e.timestamp));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let data = b"- cmd: ls -la\n  when: 1620000000\n- cmd: git status\n  when: 1620000060\n";
        let entries = parse(data).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "ls -la");
        assert_eq!(entries[0].timestamp, 1620000000);
        assert_eq!(entries[1].command, "git status");
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse(b"").is_none());
    }

    #[test]
    fn serialize_round_trip() {
        let entries = vec![HistoryEntry {
            timestamp: 100,
            command: "echo hello".to_string(),
        }];
        let data = serialize(&entries);
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed[0].command, "echo hello");
        assert_eq!(parsed[0].timestamp, 100);
    }
}
