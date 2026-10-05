use std::process::Output;

use tokio::process::Command;

use arc_core::dbg_file;

fn error_on_command_fail(output: &Output) -> anyhow::Result<()> {
    if !output.status.success() {
        Err(anyhow::anyhow!(
            "Zellij command failed with error: {:?}",
            String::from_utf8_lossy(&output.stderr)
        ))
    } else {
        Ok(())
    }
}

pub async fn session_exists(name: &str) -> anyhow::Result<bool> {
    let output = Command::new("zellij")
        .arg("list-sessions")
        .output()
        .await?;

    // zellij list-sessions exits non-zero when there are no sessions.
    if !output.status.success() {
        return Ok(false);
    }

    let output = String::from_utf8_lossy(&output.stdout);

    let exists = output
        .lines()
        .any(|line| line.split_whitespace().next() == Some(name));

    dbg_file!("Found zellij sessions, '{}' exists: {}", name, exists);

    Ok(exists)
}

pub async fn session_attach(name: &str) -> anyhow::Result<()> {
    let output = Command::new("zellij")
        .arg("attach")
        .arg(name)
        .output()
        .await?;

    error_on_command_fail(&output)?;

    Ok(())
}

pub async fn session_create(name: &str) -> anyhow::Result<()> {
    let output = Command::new("zellij")
        .arg("attach")
        .arg("--create-background")
        .arg(name)
        .output()
        .await?;

    error_on_command_fail(&output)?;

    Ok(())
}
