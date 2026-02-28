fn main() {
    // Embed the application icon into the Windows executable
    #[cfg(target_os = "windows")]
    {
        winresource::WindowsResource::new()
            .set_icon("../../assets/icons/midinet.ico")
            .set("ProductName", "MIDInet")
            .set("FileDescription", "MIDInet System Tray")
            .set("CompanyName", "Hakol Fine AV Services")
            .compile()
            .expect("Failed to compile Windows resources");
    }
}
