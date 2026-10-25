# DemaciaSportsWorldTool PR 审核记录

**审核时间**: 2026-10-14
**审核人**: DemaciaForge (德玛西亚)
**任务**: 审核并整合 11 个开放 PR

## PR 清单

| PR# | 标题 | 作者 | Head SHA | 状态 | 提交数 |
|-----|------|------|----------|------|--------|
| #41 | fix: 拆分指标修复与已验证整合内容 | DemaciaForge | 3a725bf | Open | 5 |
| #42 | fix: make fence metadata optional for submissions | DemaciaForge | c7b3a17 | Open | 2 |
| #43 | ci: split Windows tests from Android smoke | DemaciaForge | 29e7fbe | Open | 1 |
| #44 | feat: add opt-in athletics track mode | DemaciaForge | cd05b7b | Open | 2 |
| #51 | feat(track): 道路模式必经点+随机普通点强经过 | laotierocket | 41694e3 | Open | 15 |
| #52 | feat(ui): 响应式免费分发水印 | DemaciaForge | ba507dd | Open | 6 |
| #53 | 修复校园跑可选绿色电子围栏 | DemaciaForge | 4b462ed | Open | 11 |
| #57 | feat: 跨平台定位、地图选点、城市识别及路网 | (待确认) | ba2018f | Open | ? |
| #60 | fix: 保留 road 有效里程语义并补齐打卡完成标记 | DemaciaForge | 299b5b9 | Open | 3 |
| #63 | feat: 手绘路线+高德自定义路线+轨迹导入 | Autumn984 | 2544735 | Open | 10 |
| #66 | feat: leisure=track 跑道进入路网+缝合孤立连通块 | jie6886 | d385816 | Open | 1 |

## 审核进度

### 已完成初步信息收集
- ✅ PR #41: 指标修复
- ✅ PR #42: 围栏元数据可选
- ✅ PR #43: CI/测试拆分
- ✅ PR #44: 操场跑道模式
- ✅ PR #51: 必经点+随机点
- ✅ PR #52: 水印 (通过 Git 获取)
- ✅ PR #53: 绿色电子围栏
- ⏳ PR #57: 定位/地图 (待补充详情)
- ✅ PR #60: 有效里程语义
- ✅ PR #63: 自定义路线
- ✅ PR #66: leisure=track 路网

### 下一步
1. 获取每个 PR 的完整 diff
2. 识别冲突和依赖关系
3. 审核代码正确性
4. 执行测试验证
5. 修复已确认问题
6. 整合到 fork main

---

## 关键冲突与依赖关系

### 🔴 严重冲突：textlog.rs 脱敏实现分歧

**现状**：origin/main 已经包含完整的 `redact_text()` 和 `redact_pairs()` 实现（来自 PR #41）

**冲突 PR**：
- PR #42、#43、#52、#53、#63、#66 都**删除**了 `mask()` 和 `redact_json()` 函数
- 这些 PR 同时删除了对这些函数的调用，改为直接打印原始日志
- PR #41 **保留并增强**了脱敏逻辑，添加了 `redact_text()` 和 `redact_pairs()`

**根本原因**：
这些 PR 基于不同的 main 快照创建：
- PR #41 的基础快照：main 没有脱敏函数
- 其他 PR 的基础快照：main 已有 `mask()` 和 `redact_json()`
- 它们都修改了相同的 `src/api/client.rs` 和 `src/api/login.rs` 日志调用

**解决方案**：
1. **保留 PR #41 的完整脱敏实现**（已在 origin/main）
2. 其他 PR 需要 rebase 到当前 main，删除它们对 textlog.rs 的修改
3. 恢复它们删除的脱敏调用

---

### 📊 PR 功能分组

#### A 组：基础设施和工具链（无功能依赖）
- **PR #43**：CI/测试拆分 - 独立，但有 textlog 冲突
- **PR #52**：响应式水印 - 独立 UI，有 textlog 冲突

#### B 组：围栏和提交协议
- **PR #42**：围栏元数据可选 - 修改 `wire.rs`，有 textlog 冲突
- **PR #53**：绿色电子围栏 - 与 #42 功能相关，可能重复

#### C 组：轨迹生成和指标
- **PR #41**：指标修复 - 已在 main，是其他 PR 的基础
- **PR #60**：有效里程语义 - 依赖 #41 的指标修复

#### D 组：路径规划模式
- **PR #44**：操场跑道模式 - 添加新模式
- **PR #51**：必经点+随机点 - 修改道路生成逻辑
- **PR #63**：自定义路线（高德+手绘+导入）- 大型功能，修改多个模块
- **PR #66**：leisure=track 路网接入 - 修改路网加载

**潜在冲突**：
- #63 和 #66 都修改了 `crates/route-planner/src/load.rs`
- #44、#51、#63 可能在轨迹生成逻辑上冲突
- #63 添加了大量新文件（custom.rs, draw.rs, tiles.rs, amap.rs）

#### E 组：平台和定位（待确认）
- **PR #57**：跨平台定位、地图选点 - 待详细审核

---

## PR 详细审核

### PR #41: 指标修复及整合内容 ✅ 已在 main
- **作者**: DemaciaForge
- **Head SHA**: 3a725bf (审核时)
- **实际状态**: 已经整合到 origin/main
- **变更统计**: 22 文件，+979/-257 行

**核心改动**：
1. **脱敏增强** (`textlog.rs`): 添加 `redact_text()` 和 `redact_pairs()`
2. **指标修复** (`submit.rs`, `wire.rs`): pace/stride 单位修正
3. **边界约束** (`generator.rs`, `altitude.rs`): 异常点漂移 ≤6m，GPS 速度 ≤6km/h
4. **测试覆盖**: 132 个单元测试全部通过
5. **dry-run 功能** (`flow.rs`, `cmds.rs`): 添加但 CLI 未连接

**已知问题**：
- ⚠️ dry-run 功能虽然实现和测试，但 CLI 入口未连接（`cmds.rs:86`）

---

### PR #42: 围栏元数据可选 ⚠️ textlog 冲突
- **作者**: DemaciaForge
- **Head SHA**: c7b3a17
- **变更统计**: 4 文件，+98/-101 行

**核心改动**：
1. **围栏校验放宽** (`wire.rs`): 新增 `validate_fence_metadata()`
   - 允许围栏字段缺失或为空
   - 非空围栏必须有有效 `runAreaId` 和 `freedomShowFence=true`
2. **测试覆盖**: 6 个新测试覆盖各种围栏场景

**冲突**：
- 🔴 删除了 `textlog.rs` 的 97 行脱敏代码
- 🔴 删除了 `client.rs` 和 `login.rs` 的脱敏调用

**修复方案**：
- Rebase 到当前 main
- 只保留 `wire.rs` 的改动
- 恢复 `client.rs` 和 `login.rs` 的脱敏调用

---

### PR #43: CI/测试拆分 ⚠️ textlog 冲突
- **作者**: DemaciaForge
- **Head SHA**: 29e7fbe
- **变更统计**: 9 文件，+248/-114 行

**核心改动**：
1. **新工作流**: `tests.yml` (Windows/macOS/Linux)，`android-smoke.yml`
2. **构建脚本**: `android/build-linux.sh`, `android/test-linux.sh`
3. **Release 工作流调整**: `release.yml`, `apk.yml`

**冲突**：
- 🔴 与 PR #42 相同的 textlog 冲突

**修复方案**：
- Rebase 到当前 main
- 只保留 `.github/workflows/` 和 `android/` 的改动

---

### PR #44: 操场跑道模式 (待详细审核)
- **作者**: DemaciaForge  
- **Head SHA**: cd05b7b
- **提交数**: 2

---

### PR #51: 必经点+随机点 (待详细审核)
- **作者**: laotierocket
- **Head SHA**: 41694e3
- **提交数**: 15

---

### PR #52: 响应式水印 ⚠️ textlog 冲突
- **作者**: DemaciaForge
- **Head SHA**: ba507dd
- **变更统计**: 5 文件，+121/-101 行

**核心改动**：
1. **新模块** (`ui/watermark.rs`): 底部水印面板
   - 响应式布局，最大宽度 320px
   - 开源免费声明和 GitHub 链接
   - 不覆盖中央内容和按钮
2. **集成** (`ui/mod.rs`): 在主界面调用 `watermark::show()`
3. **测试**: 验证水印不遮挡状态栏和内容区域

**冲突**：
- 🔴 与 PR #42 相同的 textlog 冲突

**修复方案**：
- Rebase 到当前 main
- 只保留 `ui/watermark.rs` 和 `ui/mod.rs` 的改动

---

### PR #53: 绿色电子围栏 (待详细审核)
- **作者**: DemaciaForge
- **Head SHA**: 4b462ed
- **提交数**: 11

---

### PR #57: 定位/地图/路网 (待详细审核)
- **作者**: (待确认)
- **Head SHA**: ba2018f

---

### PR #60: 有效里程语义 (待详细审核)
- **作者**: DemaciaForge
- **Head SHA**: 299b5b9
- **提交数**: 3

---

### PR #63: 自定义路线（手绘+高德+导入）⚠️ textlog 冲突
- **作者**: Autumn984
- **Head SHA**: 2544735
- **变更统计**: 24 文件，+2879/-207 行

**核心改动**（初步）：
1. **新模块**:
   - `amap.rs`: 高德 API 集成
   - `track/custom.rs`: 自定义路线逻辑
   - `ui/draw.rs`: 手绘界面
   - `ui/tiles.rs`: 地图瓦片
2. **大规模 UI 重构**: `map.rs`, `run.rs`, `osm.rs`
3. **Android 权限**: AndroidManifest.xml

**冲突**：
- 🔴 textlog 冲突
- ⚠️ 可能与 #57 在地图/定位上冲突
- ⚠️ 可能与 #44/#51 在轨迹生成上冲突

---

### PR #66: leisure=track 路网接入 ⚠️ textlog 冲突
- **作者**: jie6886
- **Head SHA**: d385816
- **变更统计**: 4 文件，+152/-135 行

**核心改动**：
1. **路网加载增强** (`route-planner/src/load.rs`):
   - `leisure=track` way 和 relation 纳入路网
   - 新增 `stitch_components()`: 缝合孤立连通块
   - 就近连接跑道到主路网（≤30m，双向边）
2. **测试**: 连通性测试

**冲突**：
- 🔴 textlog 冲突
- ⚠️ 可能与 #63 在 `load.rs` 上冲突

---

