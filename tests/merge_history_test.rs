use opendotsync::merge::history::{HistoryEntry, three_way_merge};
use opendotsync::merge::history_bash;
use opendotsync::merge::history_zsh;
use opendotsync::merge::history_fish;

// ── Bash parser ──────────────────────────────────────────────────────────────

#[test]
fn bash_parse_fixture_ancestor() {
    let data = std::fs::read("tests/fixtures/bash_history_ancestor.txt").unwrap();
    let entries = history_bash::parse(&data).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].command, "ls -la");
    assert_eq!(entries[1].command, "cd /tmp");
    assert_eq!(entries[2].command, "git status");
}

#[test]
fn bash_parse_fixture_local() {
    let data = std::fs::read("tests/fixtures/bash_history_a.txt").unwrap();
    let entries = history_bash::parse(&data).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[3].command, "cargo build");
    assert_eq!(entries[4].command, "vim .bashrc");
}

#[test]
fn bash_parse_fixture_remote() {
    let data = std::fs::read("tests/fixtures/bash_history_b.txt").unwrap();
    let entries = history_bash::parse(&data).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[3].command, "cargo build");
    assert_eq!(entries[4].command, "ssh myserver");
}

// ── Zsh parser ───────────────────────────────────────────────────────────────

#[test]
fn zsh_parse_fixture_ancestor() {
    let data = std::fs::read("tests/fixtures/zsh_history_ancestor.txt").unwrap();
    let entries = zsh_parse_all(&data);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].command, "ls -la");
}

fn zsh_parse_all(data: &[u8]) -> Vec<HistoryEntry> {
    history_zsh::parse(data).unwrap_or_default()
}

#[test]
fn zsh_serialize_round_trip() {
    let data = std::fs::read("tests/fixtures/zsh_history_a.txt").unwrap();
    let entries = history_zsh::parse(&data).unwrap();
    let serialized = history_zsh::serialize(&entries);
    let re_parsed = history_zsh::parse(&serialized).unwrap();
    assert_eq!(entries.len(), re_parsed.len());
    for (a, b) in entries.iter().zip(re_parsed.iter()) {
        assert_eq!(a.command, b.command);
        assert_eq!(a.timestamp, b.timestamp);
    }
}

// ── Fish parser ──────────────────────────────────────────────────────────────

#[test]
fn fish_parse_basic() {
    let data = b"- cmd: ls -la\n  when: 1620000000\n- cmd: git status\n  when: 1620000060\n";
    let entries = history_fish::parse(data).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].command, "ls -la");
    assert_eq!(entries[1].command, "git status");
}

#[test]
fn fish_parse_empty_returns_none() {
    assert!(history_fish::parse(b"").is_none());
}

// ── Three-way merge algorithm ────────────────────────────────────────────────

#[test]
fn merge_no_overlap_from_fixtures() {
    let ancestor_data = std::fs::read("tests/fixtures/bash_history_ancestor.txt").unwrap();
    let local_data = std::fs::read("tests/fixtures/bash_history_a.txt").unwrap();
    let remote_data = std::fs::read("tests/fixtures/bash_history_b.txt").unwrap();

    let ancestor = history_bash::parse(&ancestor_data).unwrap();
    let local = history_bash::parse(&local_data).unwrap();
    let remote = history_bash::parse(&remote_data).unwrap();

    let merged = three_way_merge(&local, &remote, &ancestor);

    // ancestor (3) + local-only new (vim .bashrc) + remote-only new (ssh myserver)
    // cargo build is within 60s window (ts 300 vs 320), so deduplicated
    assert_eq!(merged.len(), 6);
    assert!(merged.iter().any(|e| e.command == "vim .bashrc"));
    assert!(merged.iter().any(|e| e.command == "ssh myserver"));
    assert!(merged.iter().any(|e| e.command == "cargo build"));
    // entries should be sorted by timestamp
    for i in 1..merged.len() {
        assert!(merged[i].timestamp >= merged[i - 1].timestamp);
    }
}

#[test]
fn merge_missing_ancestor_union() {
    let ancestor: Vec<HistoryEntry> = vec![];
    let local = vec![
        HistoryEntry { timestamp: 100, command: "ls".into() },
        HistoryEntry { timestamp: 200, command: "pwd".into() },
    ];
    let remote = vec![
        HistoryEntry { timestamp: 150, command: "git status".into() },
        HistoryEntry { timestamp: 250, command: "cargo build".into() },
    ];

    let merged = three_way_merge(&local, &remote, &ancestor);
    assert_eq!(merged.len(), 4);
    assert_eq!(merged[0].timestamp, 100);
    assert_eq!(merged[3].timestamp, 250);
}

#[test]
fn merge_dedup_within_60s_window() {
    let ancestor: Vec<HistoryEntry> = vec![];
    let local = vec![HistoryEntry { timestamp: 1000, command: "cargo test".into() }];
    // same command, 30 seconds later — should be deduped
    let remote = vec![HistoryEntry { timestamp: 1030, command: "cargo test".into() }];

    let merged = three_way_merge(&local, &remote, &ancestor);
    assert_eq!(merged.len(), 1, "duplicate within 60s window should be merged");
}

#[test]
fn merge_no_dedup_outside_60s_window() {
    let ancestor: Vec<HistoryEntry> = vec![];
    let local = vec![HistoryEntry { timestamp: 1000, command: "cargo test".into() }];
    // same command, 90 seconds later — should NOT be deduped
    let remote = vec![HistoryEntry { timestamp: 1090, command: "cargo test".into() }];

    let merged = three_way_merge(&local, &remote, &ancestor);
    assert_eq!(merged.len(), 2, "same command 90s apart should not be deduplicated");
}

#[test]
fn merge_empty_local_keeps_remote() {
    let ancestor = vec![HistoryEntry { timestamp: 100, command: "ls".into() }];
    let local: Vec<HistoryEntry> = vec![];
    let remote = vec![
        HistoryEntry { timestamp: 100, command: "ls".into() },
        HistoryEntry { timestamp: 200, command: "pwd".into() },
    ];

    let merged = three_way_merge(&local, &remote, &ancestor);
    // ancestor has "ls", remote added "pwd"
    assert!(merged.iter().any(|e| e.command == "pwd"));
}

#[test]
fn merge_empty_remote_keeps_local() {
    let ancestor = vec![HistoryEntry { timestamp: 100, command: "ls".into() }];
    let local = vec![
        HistoryEntry { timestamp: 100, command: "ls".into() },
        HistoryEntry { timestamp: 200, command: "vim".into() },
    ];
    let remote: Vec<HistoryEntry> = vec![];

    let merged = three_way_merge(&local, &remote, &ancestor);
    // local added "vim", remote has nothing new
    assert!(merged.iter().any(|e| e.command == "vim"));
}
