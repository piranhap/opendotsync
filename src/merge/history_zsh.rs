use super::history::HistoryEntry;

/// Parse zsh extended history: `: timestamp:elapsed;command`
/// Returns None when no valid entries found — signals fall-back to none merge.
pub fn parse(data: &[u8]) -> Option<Vec<HistoryEntry>> {
    let text = std::str::from_utf8(data).ok()?;
    let mut entries = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(": ") {
            if let Some(semi) = rest.find(';') {
                let ts_elapsed = &rest[..semi];
                let command = &rest[semi + 1..];
                if let Some(colon) = ts_elapsed.find(':') {
                    if let Ok(ts) = ts_elapsed[..colon].trim().parse::<i64>() {
                        if !command.is_empty() {
                            entries.push(HistoryEntry {
                                timestamp: ts,
                                command: command.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

/// Serialize using zsh extended history format (elapsed defaults to 0).
pub fn serialize(entries: &[HistoryEntry]) -> Vec<u8> {
    let mut out = String::new();
    for e in entries {
        out.push_str(&format!(": {}:0;{}\n", e.timestamp, e.command));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let data = b": 1620000000:0;ls -la\n: 1620000060:2;git status\n";
        let entries = parse(data).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp, 1620000000);
        assert_eq!(entries[0].command, "ls -la");
        assert_eq!(entries[1].timestamp, 1620000060);
        assert_eq!(entries[1].command, "git status");
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse(b"").is_none());
    }

    #[test]
    fn parse_no_valid_entries_returns_none() {
        assert!(parse(b"just plain text\n").is_none());
    }

    #[test]
    fn serialize_round_trip() {
        let entries = vec![
            HistoryEntry {
                timestamp: 100,
                command: "echo hi".to_string(),
            },
            HistoryEntry {
                timestamp: 200,
                command: "cargo build".to_string(),
            },
        ];
        let data = serialize(&entries);
        let parsed = parse(&data).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].command, "echo hi");
        assert_eq!(parsed[0].timestamp, 100);
    }

    #[test]
    fn command_with_semicolon() {
        let data = b": 1620000000:0;for i in 1 2 3; do echo $i; done\n";
        let entries = parse(data).unwrap();
        assert_eq!(entries[0].command, "for i in 1 2 3; do echo $i; done");
    }
}
