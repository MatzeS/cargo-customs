use anyhow::Result;
use assert_cmd::Command;

#[test]
fn test_customs_help_works_outside_of_crate() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("/tmp").arg("--help");
    cmd.assert().success();
    Ok(())
}

#[test]
fn test_customs_call_outside_of_cargo_locatin() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("/tmp");
    cmd.assert().failure();
    Ok(())
}

#[test]
fn test_customs_runs_on_single_crate() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("./tests/lonely-crate/");

    cmd.assert().success();
    Ok(())
}

#[test]
fn test_customs_runs_on_sub_folder_of_single_crate() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("./tests/lonely-crate/src");

    cmd.assert().success();
    Ok(())
}

#[test]
fn test_customs_runs_on_workspace() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("./tests/workspace");

    cmd.assert().success();
    Ok(())
}

#[test]
fn test_customs_runs_on_crate_in_workspace() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("./tests/workspace/foo");

    cmd.assert().success();
    Ok(())
}

#[test]
fn test_customs_runs_on_sub_folder_in_workspace_that_is_not_a_crate() -> Result<()> {
    let mut cmd = Command::cargo_bin("cargo-customs")?;
    cmd.current_dir("./tests/workspace/sub");

    cmd.assert().success();
    Ok(())
}

// For a workspace, it is not required that every member has a customs file, but we should warn.
