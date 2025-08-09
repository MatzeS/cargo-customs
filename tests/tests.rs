use assert_cmd::Command;

#[test]
fn test_customs_help_works_outside_of_crate() {
    let mut cmd = Command::cargo_bin("cargo-customs").unwrap();
    cmd.current_dir("/tmp").arg("--help");
    cmd.assert().success();
}

// Error when invoked outtside of crate

#[test]
fn test_customs_runs_on_single_crate() {
    let mut cmd = Command::cargo_bin("cargo-customs").unwrap();
    cmd.current_dir("./tests/lonely-crate/");

    cmd.assert().success();
}
