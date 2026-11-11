use std::process::Command;

#[test]
fn generates_shell_completions_without_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let missing_config = temp.path().join("missing.yaml");

    for (shell, registration) in [
        ("bash", "complete -F _opendotsync"),
        ("zsh", "#compdef opendotsync"),
        ("fish", "complete -c opendotsync"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_opendotsync"))
            .arg("--config")
            .arg(&missing_config)
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(output.status.success(), "{shell}: {output:?}");
        assert!(output.stderr.is_empty(), "{shell}: {output:?}");
        let script = String::from_utf8(output.stdout).unwrap();
        for expected in [registration, "push", "no-auto-pull", "accept-remote"] {
            assert!(script.contains(expected), "{shell}: missing {expected}");
        }
    }
    assert!(!missing_config.exists());
}

#[test]
fn completions_command_is_hidden_from_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_opendotsync"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.contains("completions"));
}

#[test]
fn rejects_missing_or_invalid_shell() {
    for args in [vec!["completions"], vec!["completions", "invalid"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_opendotsync"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}
