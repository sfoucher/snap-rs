fn main() {
    #[cfg(windows)]
    embed_version_info();
}

/// Product name and version in the exe, so signing services can check them.
#[cfg(windows)]
fn embed_version_info() {
    use std::{env, fs, path::PathBuf};
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let n: Vec<&str> = version
        .split(['.', '-', '+'])
        .chain(["0", "0", "0"])
        .take(3)
        .collect();
    let rc = format!(
        r#"1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "ProductName", "snap-rs"
      VALUE "ProductVersion", "{version}"
      VALUE "FileDescription", "snap-rs image viewer"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "snap-gui"
      VALUE "OriginalFilename", "snap-gui.exe"
      VALUE "LegalCopyright", "Copyright (c) 2026 Samuel Foucher"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        a = n[0],
        b = n[1],
        c = n[2],
    );
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("snap-gui.rc");
    fs::write(&path, rc).unwrap();
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
