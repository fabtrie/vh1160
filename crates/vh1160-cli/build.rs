//! Windows resources of `vh1160-cli.exe`: the app's icon (shown in Explorer
//! and the file properties; a console window keeps its terminal's icon) and
//! the version info. The icon itself lives with the app: app/assets/icon.

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = Path::new(&manifest_dir).join("../../app/assets/icon/vh1160.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", icon.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // rc.exe string literals: forward slashes need no escaping.
    let icon = icon.to_string_lossy().replace('\\', "/");
    let v = |k: &str| std::env::var(k).unwrap_or_else(|_| "0".into());
    let (major, minor, patch) = (
        v("CARGO_PKG_VERSION_MAJOR"),
        v("CARGO_PKG_VERSION_MINOR"),
        v("CARGO_PKG_VERSION_PATCH"),
    );
    let version = v("CARGO_PKG_VERSION");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "VH1160 command-line control"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "vh1160-cli"
      VALUE "OriginalFilename", "vh1160-cli.exe"
      VALUE "ProductName", "VH1160 Control"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("vh1160-cli.rc");
    std::fs::write(&out, rc).unwrap();
    embed_resource::compile_for(&out, ["vh1160-cli"], embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
