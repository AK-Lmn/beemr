//! Embeds the icon and version information (product name, version,
//! description) into the Windows executable. Does nothing on other platforms.

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/beemr.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/beemr.ico")
            .set("ProductName", "beemr")
            .set(
                "FileDescription",
                "beemr - peer-to-peer file and message sharing",
            )
            .set("CompanyName", "beemr")
            .set(
                "LegalCopyright",
                "Copyright (c) 2026 The beemr contributors. MIT License.",
            )
            .set("OriginalFilename", "beemr.exe")
            .set("InternalName", "beemr");
        resource
            .compile()
            .expect("failed to embed Windows resources");
    }
}
