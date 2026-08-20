fn main() {
    // tauri-build 嵌入 exe 图标资源时未声明 rerun-if-changed，
    // 只改图标文件不会触发重链。这里补上跟踪，保证 `npm run icons` 后重新构建即生效。
    // 图标文件来源是 tauri.conf.json 的 bundle.icon（bundle 未激活，但 tauri-build
    // 仍从中取 .ico 嵌入 exe），因此 bundle.icon 列表不能删除。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/favicon-source.svg");
    // tauri-codegen 嵌入 frontendDist（../ui）资源时同样没有跟踪声明：
    // 补上目录级跟踪，改 UI 文件后 release 构建自动重新嵌入资源
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=app.manifest.xml");
    // 预设插件清单：改 resources/preset-plugins.json 后重新构建即生效
    println!("cargo:rerun-if-changed=resources/preset-plugins.json");
    // 自定义 Windows 应用清单：默认清单缺 dpiAwareness，高 DPI 屏（150%+）
    // 下窗口尺寸被系统虚拟化（逻辑像素按 96 DPI 解释），弹窗/主窗口偏小
    let mut windows = tauri_build::WindowsAttributes::new();
    windows = windows.app_manifest(include_str!("app.manifest.xml"));
    if let Err(error) =
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
    {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
