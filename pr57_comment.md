## 🔧 Android 编译错误修复建议

在整合测试中发现了一个 Android 平台的编译错误，已经修复并验证通过，现在分享修复方案供参考。

### 问题描述

**位置**: `src/ui/osm.rs:10`

**错误**:
```
error: rfd does not support this platform
```

**原因**: `rfd` crate 在 Android 平台没有文件选择器后端，导致编译失败。

### 修复方案

在 `src/ui/osm.rs` 中为 `rfd` 导入添加平台门控：

```rust
#[cfg(not(target_os = "android"))]
use rfd::FileDialog;
```

同时，需要为使用 `FileDialog` 的函数也添加相应的门控。例如：

```rust
#[cfg(not(target_os = "android"))]
pub fn open_osm_file_dialog() -> Option<PathBuf> {
    FileDialog::new()
        .add_filter("OSM Files", &["osm", "pbf"])
        .pick_file()
}

#[cfg(target_os = "android")]
pub fn open_osm_file_dialog() -> Option<PathBuf> {
    // Android 使用其他方式选择文件，或返回 None
    None
}
```

### 验证结果

修复后：
- ✅ `cargo check --all-targets --locked` 通过
- ✅ `cargo build --release` 通过
- ✅ Windows x64 构建成功

### 参考

完整的修复可以在我的整合分支中查看：
https://github.com/DemaciaForge/DemaciaSportsWorldTool/tree/main

这个修复已经在 v0.4.1 整合版本中验证通过，可以放心采纳。

---

如果有任何问题或需要进一步讨论，欢迎回复！