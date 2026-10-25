# v0.4.1 非官方构建

**警告：这是非官方构建版本，不代表上游仓库 DemaciaForge/DemaciaSportsWorldTool 的官方发布。**

## 概述

本版本完整整合了上游 11 个 PR 的功能改进，涵盖指标修复、可选围栏、测试治理、多种跑步模式、路网系统及自定义路线等特性。

## 整合的 PR 列表

### ✅ 完整采用（8个）

1. **PR #41** - 指标修复及相关整合内容
   - 作者: YanamiNeko
   - 修复运动指标计算错误
   - 添加 CLI dry-run fixtures
   - 修复状态：已在整合中修复少量问题

2. **PR #42** - 提交时围栏元数据可选
   - 作者: YanamiNeko
   - 围栏元数据改为可选，支持无围栏提交
   - 修复状态：已修复围栏解析和载荷一致性问题

3. **PR #43** - Windows/Android 测试拆分与发布治理
   - 作者: YanamiNeko
   - CI 工作流优化，分离烟雾测试和发布流程
   - 修复状态：无问题

4. **PR #44** - 可选操场跑道模式
   - 作者: YanamiNeko
   - 添加 athletics track 操场模式
   - 修复状态：已修复异常点边界测试

5. **PR #52** - 响应式免费分发水印
   - 作者: YanamiNeko
   - 添加开源防倒卖水印
   - 修复状态：已修复水印文本和布局

6. **PR #53** - 可选绿色电子围栏
   - 作者: YanamiNeko
   - 支持绿色电子围栏协议
   - 修复状态：已验证围栏载荷格式

7. **PR #60** - road 有效里程语义与打卡完成标记
   - 作者: YanamiNeko
   - 道路模式有效里程计算和完成标记
   - 修复状态：无问题

8. **PR #66** - leisure=track 跑道接入路网及孤立连通块缝合
   - 作者: Autumn984
   - OSM leisure=track 跑道识别
   - 孤立连通块缝合算法
   - 修复状态：无问题

### ⚠️ 部分采用（3个）

9. **PR #51** - 道路模式必经点、随机普通点强经过
   - 作者: Scien-Moon
   - 采用：必经点强制经过逻辑
   - 排除：与 PR #60 冲突的有效里程计算
   - 修复状态：已协调与其他 PR 的语义冲突

10. **PR #57** - 跨平台定位、地图选点、城市识别及路网
    - 作者: laotierocket
    - 采用：定位功能、地图选点 UI、城市识别、路网加载核心
    - 排除：与 PR #66 冲突的路网缝合方案
    - 修复状态：已修复 Android 配置和 rfd 依赖门控

11. **PR #63** - 自定义路线：高德步行路径及绕圈/圈数
    - 作者: Autumn984
    - 采用：高德步行路径、圈数配置、UI 集成
    - 排除：与现有模式冲突的默认行为
    - 修复状态：已修复 `cmd_run` 缺失的 client 初始化和测试字段

## 技术细节

### 修复的主要问题

1. **PR #42/53 围栏处理**
   - 修复绿色围栏载荷格式匹配
   - 修复可选围栏时的 area_id 恢复逻辑

2. **PR #51/60 道路模式语义冲突**
   - 协调必经点强制经过与有效里程计算
   - 保留两者的独立功能

3. **PR #57 跨平台兼容性**
   - 修复 `rfd` 在 Android 上的编译错误
   - 添加 `cfg(not(target_os = "android"))` 门控

4. **PR #57/66 路网缝合方案选择**
   - 采用 PR #66 的基于 MST 的缝合算法
   - 排除 PR #57 的简单最近点连接方案

5. **PR #63 CLI 运行命令**
   - 修复 `cmd_run` 中缺失的 `client`、`session` 变量初始化
   - 修复测试中 `RunParams` 缺失的 `custom` 和 `amap_key` 字段

### 合并冲突解决

- **Cargo.toml**: 协调版本号和依赖配置
- **src/ui/map.rs**: 整合地图 UI 和路网加载
- **src/api/flow.rs**: 协调运行参数结构
- **crates/route-planner/src/load.rs**: 整合路网加载和缝合逻辑

### 测试验证

- ✅ `cargo check --all-targets --locked`
- ✅ `cargo test --all --locked` (142 tests passed)
- ✅ `cargo build --release --locked`
- ✅ Windows x64 GUI 模式
- ⚠️ macOS/Linux/Android 未在本地验证（需 CI 或真机）

## 已知限制

1. **平台验证范围**
   - 仅在 Windows x64 本地验证
   - macOS、Linux、Android 需等待 CI 或真机测试

2. **协议兼容性**
   - 绿色围栏、高德路径等特性依赖服务端支持
   - 未进行真实账号提交测试

3. **上游状态**
   - 这些 PR 在上游仍为 open 状态
   - 上游可能有不同的合并决策

## 下载

**文件**: DemaciaSportsWorldTool-v0.4.1-windows-x64.tar.gz  
**SHA256**: `b28264fdc73944dd424a2f19b5a218da8ccfc07287396a292404adc3beade282`

## 构建信息

- **构建时间**: 2026-10-14
- **Rust 版本**: (系统默认)
- **目标平台**: x86_64-pc-windows-msvc
- **优化级别**: release (opt-level=3, lto=thin, strip=true)

## 致谢

感谢以下贡献者的原始工作：
- YanamiNeko (PR #41, #42, #43, #44, #52, #53, #60)
- Autumn984 (PR #63, #66)
- Scien-Moon (PR #51)
- laotierocket (PR #57)

---

**再次提醒：这是非官方整合版本，请以上游官方发布为准。**
