fn main() {
    // Embed the app icon into the Windows executable (shown in Explorer / taskbar).
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/file.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        // The icon of files associated with nanote (`file_assoc::FILE_ICON_ID`).
        res.set_icon_with_id("assets/file.ico", "2");
        res.set("ProductName", "nanote");
        res.set("FileDescription", "nanote");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed icon: {e}");
        }
    }
}
