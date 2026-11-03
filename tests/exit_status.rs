//! Exit statuses of the built binary, as scripts see them.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_contact-sheet"))
        .args(args)
        .output()
        .expect("run contact-sheet")
}

#[test]
fn invalid_options_exit_1_not_2() {
    // 2 means "sheets written, some files skipped"; a usage error writes nothing.
    for args in [
        &["--dpi", "1200", "photos"][..],
        &["--bogus", "photos"],
        &["--paper", "a5", "photos"],
        &[],
    ] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(1), "args {args:?}");
        assert!(!out.stderr.is_empty(), "args {args:?}: no message on stderr");
    }
}

#[test]
fn help_and_version_exit_0_on_stdout() {
    for flag in ["--help", "--version"] {
        let out = run(&[flag]);
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert!(!out.stdout.is_empty(), "{flag}: nothing on stdout");
    }
}
