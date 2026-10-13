# PR 审核记录 - v0.4.0 整合

**审核时间**: 2026-10-13
**上游 main SHA**: `6b677e1b4f4f6e0bf7a287a287d937ead5048800`
**Fork main SHA**: `eccbc025281bb36a9dfedc04c6a8f74856a5b4fd`
**目标版本**: v0.4.0

---

## PR 清单概览

| PR# | 作者 | Head SHA | 提交数 | 文件数 | 标题 |
|-----|------|----------|--------|--------|------|
| #41 | DemaciaForge | 3a725bf | 5 | 22 | 拆分指标修复与已验证整合内容 |
| #42 | DemaciaForge | c7b3a17 | 2 | 1 | 围栏元数据可选 |
| #43 | DemaciaForge | 29e7fbe | 1 | 6 | CI 拆分与发布治理 |
| #44 | DemaciaForge | cd05b7b | 2 | 8 | 操场跑道模式 |
| #51 | laotierocket | 97b31dc | 14 | 13 | 道路必经点+随机普通点 |
| #52 | DemaciaForge | ba507dd | 6 | 2 | 响应式水印 |
| #53 | DemaciaForge | 4b462ed | 11 | 10 | 可选绿色电子围栏 |
| #57 | LightWhite520 | ba2018f | 6 | 35 | 跨平台定位地图选点 |
| #60 | DemaciaForge | 299b5b9 | 3 | 14 | road 有效里程与完成标记 |
| #63 | Autumn984 | 0fb6ca6 | 6 | 21 | 自定义路线高德路径 |
| #66 | jie6886 | d385816 | 1 | 1 | leisure=track 跑道路网 |

---

## PR #42: 围栏元数据可选

**审核 head**: `c7b3a17cd3141c37ee14ca89ceb56f021440337c`

### 实际变更
- 文件: `src/track/wire.rs`
- 新增 `validate_fence_metadata()` 函数，从原 `validate_five_point_wrapper()` 中提取围栏验证逻辑
- 围栏字段 (`geoFencesJson`, `runAreaId`, `freedomShowFence`) 缺失或为空时不报错
- 有围栏数据时强制检查 `runAreaId >= -1` 和 `freedomShowFence == true`
- 新增 6 个测试用例覆盖：缺失字段、空数组、null、有效围栏、隐藏围栏、格式错误

### 测试证据
✅ 6 个新增单元测试覆盖所有分支
✅ 允许无围栏提交（自由模式）
✅ 拒绝非空围栏但 `freedomShowFence=false`（防止隐藏围栏绕过）

### 已确认问题
无

### 待确认风险
无

### 与其他 PR 关系
- **互补 #53**: #42 放宽提交验证允许无围栏；#53 在 UI 层添加可选绿色围栏功能
- 不冲突：#42 是协议层验证，#53 是 UI 配置

### 审核结论
✅ **可完整采用** - 逻辑正确，测试充分，向后兼容

---

## PR #43: CI 拆分与发布治理

**审核 head**: `29e7fbe7580fcf740a4d708ab5a57cdab1a6d62c`

### 实际变更
1. **新增 Windows Tests 工作流** (`.github/workflows/tests.yml`):
   - PR 和 main push 触发
   - Windows 平台 `cargo test --all --locked`
   - 保存测试日志到 artifact

2. **新增 Android Smoke 工作流** (`.github/workflows/android-smoke.yml`):
   - 每周一 03:17 UTC 或手动触发
   - 构建 x86_64 模拟器 APK
   - 启动检查 70 秒无崩溃
   - Activity 重建和进程持久化测试

3. **发布权限治理** (`apk.yml`, `release.yml`):
   - 发布条件从 `startsWith(github.ref, 'refs/tags/')` 改为
     `github.event_name == 'push' && startsWith(github.ref, 'refs/tags/') && github.repository == 'YanamiNeko/NekoSportsWorldTool'`
   - fork 和非 push 事件只上传 artifact，不创建 Release
   - 上游 tag 发布仍需 `ANDROID_KEYSTORE_BASE64` Secret，fork 可用临时密钥

4. **Linux 构建脚本** (`android/build-linux.sh`, `android/test-linux.sh`):
   - 隔离的 CI 构建和测试脚本
   - 16KB 对齐验证
   - 启动检查、logcat 和 dumpsys 诊断

### 测试证据
✅ 新增完整的 Android smoke 测试框架
✅ Windows 测试工作流分离
✅ 发布权限正确隔离上游/fork

### 已确认问题
无

### 待确认风险
- Android emulator 在 CI 中可能不稳定（超时设置 45 分钟）
- 需要实际运行 CI 验证工作流配置正确

### 与其他 PR 关系
- **互补 #57**: #57 引入 Android 定位和地图功能，#43 的 smoke 测试会验证 Android 编译和启动
- 不冲突：基础设施层改动

### 审核结论
✅ **可完整采用** - CI 治理合理，测试覆盖完善，发布权限正确隔离

---

## PR #52: 响应式水印

**审核 head**: `ba507ddf4f01af66fa290ad041c4e28baab2711b`

### 实际变更
- 文件: `src/ui/mod.rs`, `src/ui/watermark.rs`
- 新增独立水印模块，在底部面板显示开源信息
- 水印内容：免费提供、禁止商用、GitHub 链接、贡献者链接
- 使用 `TopBottomPanel::bottom` 参与正常布局，不覆盖内容
- 响应式：最大宽度 320px 或可用宽度较小值
- 新增单元测试验证水印不覆盖状态栏和中央内容

### 测试证据
✅ 单元测试覆盖多种屏幕宽度 (320/390/1280px)
✅ 验证水印位于状态栏上方、内容下方

### 已确认问题
无

### 待确认风险
无 - UI 层改动，不影响核心功能

### 与其他 PR 关系
- **可能与 #57 交互**: #57 引入地图控件，需要验证水印不遮挡地图底部按钮
- 不冲突：独立的 UI 层

### 审核结论
✅ **可完整采用** - UI 实现合理，测试覆盖充分

---

## PR #44: 操场跑道模式

**审核 head**: `cd05b7baa6920a7be89587d4876e49030186f4c9`

### 实际变更
1. **新增标准跑道模块** (`src/track/stadium.rs`):
   - `TrackSpec` 枚举：Auto / M200 / M300 / M400 / Custom
   - 标准田径场几何：直道 84.39m，半径 36.80m，总长 400m
   - 其他规格按比例缩放
   - `make_stadium_ring()`: 拟合标准椭圆到服务器打卡点
   - 旋转拟合算法，180 度采样找最佳角度和偏移
   - 容错 30% 半径 + 3m，超出拒绝
   - 服务器点位 3m 内直接吸附到轨迹

2. **集成到生成器** (`src/track/generator.rs`):
   - `build_stadium()`: 标准跑道模式入口
   - `build_on_ring()`: 重构以支持可选预设轨迹环

3. **配置和 UI** (`src/api/model.rs`, `src/ui/run.rs`):
   - Config 新增 `track_spec` 字段，默认 Auto
   - UI 新增操场规格下拉框（Auto / 200m / 300m / 400m / 自定义）
   - 自定义模式支持 100-1000m 范围
   - 选择标准跑道时隐藏道路预览

4. **流程整合** (`src/api/flow.rs`):
   - `RunParams` 新增 `track_spec` 字段
   - 非 Auto 模式优先使用 `build_stadium()`

### 测试证据
✅ 单元测试覆盖：
  - 200/300/400m 标准长度验证
  - 拟合环保持名义长度和检查点方向
  - 生成轨迹一致性检查
  - 拒绝不匹配的点位

### 已确认问题
无

### 待确认风险
- **未验证**: 标准跑道能否通过服务器端点位验证（需要真实提交测试）
- **容错参数**: 30% 半径 + 3m 的容错值是经验值，可能有学校的打卡点偏离更大

### 与其他 PR 关系
- **不冲突 #63**: #63 是自定义路线（高德 API），#44 是标准椭圆跑道，两个独立模式
- **不冲突 #66**: #66 是 OSM leisure=track 路网整合，#44 是几何生成，不依赖路网
- **不冲突道路模式**: Auto 保留原有行为

### 审核结论
✅ **可完整采用** - 设计合理，测试充分，不破坏现有功能

---

## PR #66: leisure=track 跑道路网

**审核 head**: `d385816616206e277e410338d85d70f9f46e0845`

### 实际变更
