//! `cargo xtask install-signed --as` refuses, with a usage exit, a name that
//! cannot be a command, before anything is built or installed.

mod support;

#[test]
fn install_signed_refuses_a_name_that_is_not_a_command_name_with_exit_2() {
    let scratch = std::env::temp_dir().join(format!("xtask-install-{}", std::process::id()));
    let home = scratch.join("cargo");
    std::fs::create_dir_all(&home).unwrap();

    // Nothing may land in the scratch CARGO_HOME, and nothing in the real one.
    let output = support::xtask(&scratch)
        .args(["install-signed", "--as", "../kurama"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("`--as ../kurama`"), "{stderr}");
    assert!(std::fs::read_dir(&home).unwrap().next().is_none());
    std::fs::remove_dir_all(&scratch).unwrap();
}
