//! Embeds the icon, version info and application manifest into the Windows executable.
//!
//! Release builds request administrator rights (needed for S.M.A.R.T. and raw
//! drive access). Debug and test builds stay `asInvoker` so `cargo run` and
//! `cargo test` work from a normal terminal.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let level = if std::env::var("PROFILE").as_deref() == Ok("release") {
        "requireAdministrator"
    } else {
        "asInvoker"
    };
    let manifest = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="{level}" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
</assembly>
"#
    );

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set_manifest(&manifest)
        .set("ProductName", "DiskTools")
        .set("FileDescription", "DiskTools — disk benchmark and health monitor")
        .set("CompanyName", "arizlab")
        .set("LegalCopyright", "© arizlab")
        .set("OriginalFilename", "disktools.exe");
    if let Err(e) = res.compile() {
        panic!("failed to embed Windows resources: {e}");
    }
}
