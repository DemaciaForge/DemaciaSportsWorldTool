# v0.4.1 PR 整合简明对照表

## 快速概览

| PR# | 标题 | 作者 | 状态 | 修复提交 | 评论/PR |
|-----|------|------|------|---------|---------|
| #41 | 指标修复及相关整合内容 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #42 | 提交时围栏元数据可选 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #43 | Windows/Android 测试拆分与发布治理 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #44 | 可选操场跑道模式 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #51 | 道路模式必经点、随机普通点强经过 | Scien-Moon | ⚠️ 部分采用 | - | 需协调 #60 |
| #52 | 响应式免费分发水印 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #53 | 可选绿色电子围栏 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #57 | 跨平台定位、地图选点、城市识别及路网 | laotierocket | ⚠️ 部分采用 | Android cfg | 需协调 #66 |
| #60 | road 有效里程语义与打卡完成标记 | YanamiNeko | ✅ 完整采用 | - | 已在上游 |
| #63 | 自定义路线：高德步行路径及绕圈/圈数 | Autumn984 | ✅ 完整采用 | CLI + 测试 | d653530 |
| #66 | leisure=track 跑道接入路网及孤立连通块缝合 | Autumn984 | ✅ 完整采用 | - | e9d9214 |

**统计**:
- 完整采用: 8 个
- 部分采用: 3 个
- 暂不采用: 0 个
- 修复的 PR: 2 个 (#57, #63)

---

## 重复与依赖关系

### 部分重复
- **#51 vs #60**: 道路有效里程计算 → 采用 #60
- **#57 vs #66**: 路网缝合算法 → 采用 #66

### 互补关系
- **#42 + #53**: 可选围栏 + 绿色围栏
- **#51 + #60**: 必经点 + 有效里程
- **#57 + #66**: 路网加载 + 缝合优化

---

## 已修复问题

### PR #57
- **问题**: Android 编译失败，`rfd` 无后端
- **位置**: `src/ui/osm.rs:10`
- **修复**: 添加 `#[cfg(not(target_os = "android"))]`
- **提交**: 已整合到 main

### PR #63
1. **问题**: CLI `client` 变量未定义
   - **位置**: `src/cli/cmds.rs:219`
   - **修复**: 添加 client/session/identity 初始化
   - **提交**: d653530

2. **问题**: 测试 `RunParams` 缺少字段
   - **位置**: `src/api/flow.rs:686`
   - **修复**: 添加 `custom` 和 `amap_key` 字段
   - **提交**: d653530

---

## 验证结果

### 编译与测试
- ✅ `cargo check --all-targets --locked`
- ✅ `cargo test --all --locked` (142/142)
- ✅ `cargo build --release --locked`
- ✅ Windows x64 运行验证

### 平台覆盖
- ✅ Windows x64: 完整验证
- ⚠️ macOS: 需 CI
- ⚠️ Linux: 需 CI
- ⚠️ Android: 需真机

---

## 发布信息

- **版本**: v0.4.1
- **提交**: 1c68fcf
- **标签**: https://github.com/DemaciaForge/DemaciaSportsWorldTool/releases/tag/v0.4.1
- **下载**: DemaciaSportsWorldTool-v0.4.1-windows-x64.tar.gz (4.1 MB)
- **SHA256**: `b28264fdc73944dd424a2f19b5a218da8ccfc07287396a292404adc3beade282`

---

## 上游合并状态

### 可直接合并 (2个)
- #63: 需先采纳 fork 的编译修复
- #66: 无问题

### 需协调 (2个)
- #51: 与 #60 协调有效里程计算
- #57: 与 #66 协调路网缝合算法

### 已在上游 (7个)
- #41, #42, #43, #44, #52, #53, #60

---

**详细报告**: 见 `INTEGRATION_REPORT_v0.4.1.md`  
**Release Notes**: 见 `RELEASE_NOTES_v0.4.1.md`
