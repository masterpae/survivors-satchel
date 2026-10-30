fn main() {
    slint_build::compile("ui/app.slint").expect("the Slint UI failed to compile");
    // The commit the exe is built from, for Settings' About (the GitHub workflows pass it); without
    // it the exe calls itself a local build. The repository it is released from: `about.rs` reads
    // it at compile time, so a change rebuilds.
    println!("cargo:rerun-if-env-changed=SATCHEL_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=SATCHEL_REPOSITORY");
    if let Ok(commit) = std::env::var("SATCHEL_GIT_COMMIT") {
        println!("cargo:rustc-env=SATCHEL_COMMIT={commit}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resource();
    }
}

/// The exe's own icon and version, as Windows shows them (Explorer, Task Manager): a resource
/// compiled with `llvm-rc` (in the build image) and handed to the linker as it is. Where
/// `llvm-rc` is missing the exe is built without them.
fn windows_resource() {
    use std::path::PathBuf;
    use std::process::Command;
    println!("cargo:rerun-if-changed=ui/icons/satchel.ico");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("ui/icons/satchel.ico");
    let number = |part: &str| std::env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap_or_else(|_| "0".into());
    let (major, minor, patch) = (number("MAJOR"), number("MINOR"), number("PATCH"));
    let version = env!("CARGO_PKG_VERSION");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Survivor's Satchel"
      VALUE "ProductName", "Survivor's Satchel"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "SurvivorsSatchel.exe"
      VALUE "LegalCopyright", "MIT licence"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
    );
    let rc = out.join("satchel.rc");
    let res = out.join("satchel.res");
    std::fs::write(&rc, script).expect("the resource script could not be written");
    // Run inside OUT_DIR with relative names: llvm-rc reads an argument starting with `/` as an
    // option (`/cache/…` would be its `/c` option). The script has no #include or macro, so it
    // needs no preprocessor either.
    match Command::new("llvm-rc").current_dir(&out).args(["-no-preprocess", "/fo", "satchel.res", "satchel.rc"]).output() {
        Ok(run) if run.status.success() => println!("cargo:rustc-link-arg-bins={}", res.display()),
        // Without its version resource no other Satchel could offer this exe as an update.
        Ok(run) => panic!("llvm-rc failed ({}): {}", run.status, String::from_utf8_lossy(&run.stderr)),
        Err(e) => panic!("llvm-rc not found ({e}): the exe would have no icon and no version"),
    }
}
