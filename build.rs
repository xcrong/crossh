fn main() {
    // Windows 资源嵌入：把 assets/appicon/AppIcon.ico 嵌进所有 Windows 二进制
    // （crossh.exe / crossh-git.exe / crossh-note.exe / crossh-updater.exe）。
    //
    // embed-resource 在非 Windows target 时自动 no-op，所以 Linux/macOS 构建
    // 无需 windres/rc.exe；Windows 上（包括 CI 的 windows-latest）会在编译时
    // 调用 RC.EXE / llvm-rc / windres 完成嵌入。
    println!("cargo:rerun-if-changed=assets/appicon/app.rc");
    println!("cargo:rerun-if-changed=assets/appicon/AppIcon.ico");
    println!("cargo:rerun-if-changed=assets/appicon/icon-master.svg");

    // 新版 embed-resource 3.x 要求处理 CompilationResult
    let _ = embed_resource::compile("assets/appicon/app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
