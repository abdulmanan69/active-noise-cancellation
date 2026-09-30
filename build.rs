//! Embeds the application icon and version information into clearmic.exe.
//! Optional: when `windres` or the icon is unavailable the build continues without them.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_windres() -> Option<PathBuf> {
    if let Ok(p) = env::var("CLEARMIC_WINDRES") {
        return Some(PathBuf::from(p));
    }
    if let Ok(home) = env::var("USERPROFILE") {
        let p = Path::new(&home).join(".clearmic-build/w64devkit/bin/windres.exe");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/clearmic.ico");
    println!("cargo:rerun-if-env-changed=CLEARMIC_WINDRES");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("assets").join("clearmic.ico");
    let Some(windres) = find_windres() else {
        println!("cargo:warning=windres not found; building without embedded icon");
        return;
    };
    if !ico.exists() {
        println!("cargo:warning=assets/clearmic.ico missing; run `clearmic --export-icon`");
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (a, b, c) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc = out.join("clearmic.rc");
    let obj = out.join("clearmic_res.o");
    let ico_path = ico.to_string_lossy().replace('\\', "/");
    let script = format!(
        r#"1 ICON "{ico_path}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "ClearMic\0"
      VALUE "FileDescription", "ClearMic - AI noise cancellation\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "clearmic\0"
      VALUE "OriginalFilename", "clearmic.exe\0"
      VALUE "ProductName", "ClearMic\0"
      VALUE "ProductVersion", "{version}\0"
      VALUE "LegalCopyright", "MIT License\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    if std::fs::write(&rc, script).is_err() {
        println!("cargo:warning=could not write resource script");
        return;
    }
    let status = Command::new(&windres)
        .arg("-i")
        .arg(&rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", obj.display());
        }
        other => println!("cargo:warning=windres failed ({other:?}); building without icon"),
    }
}
