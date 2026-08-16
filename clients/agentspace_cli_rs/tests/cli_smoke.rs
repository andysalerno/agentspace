use std::{
    error::Error,
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
fn memory_write_and_read_work_through_unified_binary() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentspace"))
        .args([
            "memory",
            "--root",
            root.path().to_str().ok_or("temp path is not UTF-8")?,
            "write",
            "projects/example",
            "--title",
            "Example",
            "--tag",
            "project",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing stdin")?
        .write_all(b"Durable fact.\n")?;
    let write = child.wait_with_output()?;
    assert!(
        write.status.success(),
        "write failed: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    let read = Command::new(env!("CARGO_BIN_EXE_agentspace"))
        .args([
            "memory",
            "--root",
            root.path().to_str().ok_or("temp path is not UTF-8")?,
            "read",
            "projects/example",
        ])
        .output()?;
    assert!(
        read.status.success(),
        "read failed: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(String::from_utf8(read.stdout)?.contains("Durable fact."));
    Ok(())
}
