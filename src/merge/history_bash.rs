use super::history::HistoryEntry;

/// Parse bash timestamped history (`HISTTIMEFORMAT` enabled).
/// Returns None when no timestamped entries are found — signals fall-back to none merge.
pub fn parse(data: &[u8]) -> Option<Vec<HistoryEntry>> {
    let text = std::str::from_utf8(data).ok()?;
    let mut entries = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(ts_str) = line.strip_prefix('#') {
            if let Ok(ts) = ts_str.trim().parse::<i64>() {
                if let Some(cmd) = lines.next() {
                    if !cmd.trim().is_empty() {
                        entries.push(HistoryEntry {
                            timestamp: ts,
                            command: cmd.to_string(),
                        });
                    }
                }
            }
            // skip non-timestamp comment lines
        }
        // skip lines not preceded by a timestamp (pre-HISTTIMEFORMAT content)
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
        out.push('#');
        out.push_str(&e.timestamp.to_string());
        out.push('\n');
        out.push_str(&e.command);
        out.push('\n');
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let data = b"#1620000000\nls -la\n#1620000060\ngit status\n";
        let entries = parse(data).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp, 1620000000);
        assert_eq!(entries[0].command, "ls -la");
        assert_eq!(entries[1].timestamp, 1620000060);
        assert_eq!(entries[1].command, "git status");
    }

    #[test]
    fn parse_skips_comment_lines() {
        let data = b"# this is a comment\n#1620000000\nls\n";
        let entries = parse(data).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "ls");
    }

    #[test]
    fn parse_no_timestamps_returns_none() {
        let data = b"ls -la\ngit status\n";
        assert!(parse(data).is_none());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse(b"").is_none());
    }

    #[test]
    fn serialize_round_trip() {
        let entries = vec![
            HistoryEntry { timestamp: 100, command: "echo hello".to_string() },
            HistoryEntry { timestamp: 200, command: "pwd".to_string() },
        ];
        let data = serialize(&entries);
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed, entries);
    }
}
