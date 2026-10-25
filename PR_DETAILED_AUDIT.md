# PR 详细审核报告

审核时间：2026-10-14
审核基准：origin/main (69856c9)

## PR 概览与目标

### PR #41 - 指标修复及日志脱敏（DemaciaForge）
**Head SHA**: 3a725bf
**状态**: OPEN, CONFLICTING
**目标**:
- 新增日志脱敏功能（token/敏感字段）
- 新增 `run --dry-run` 本地校验模式
- 新增测试 fixtures 和单元测试
- 修复代码格式和质量

**修改文件** (22个):
- 核心：textlog.rs (+203), api/flow.rs (+184), api/client.rs (+69)
- 测试：新增 3 个 fixture JSON
- 其他：policy.rs, rank.rs, records.rs, login.rs, submit.rs, generator.rs 等

**实际行为**:
- `run --dry-run`: 不登录、不联网，构造完整轨迹并校验五点/OBS/速度窗口
- 日志脱敏：自动替换 token/uuid/手机号等敏感信息为 `***`
- 测试增强：data 字段字符串/对象兼容、业务错误脱敏

---

### PR #42 - 围栏元数据可选（DemaciaForge）
**Head SHA**: c7b3a17
**状态**: OPEN, UNKNOWN
**目标**: 允许提交时围栏元数据缺失/null/空，但存在时验证格式

**修改文件** (1个): src/track/wire.rs

**实际行为**:
- 新增 `validate_fence_metadata()` 函数
- 围栏缺失/null/空数组 → 通过
- 围栏存在但格式错误 → 拒绝
- 围栏存在但 freedomShowFence=false → 拒绝
- 新增 6 个单元测试覆盖各种状态

**测试证据**: cargo test 89 个测试通过

---

### PR #43 - CI 拆分与发布治理（DemaciaForge）
**Head SHA**: 29e7fbe
**状态**: OPEN, UNKNOWN
**目标**: Windows 测试与 Android smoke 拆分，规范发布流程

**修改文件** (6个): 
- .github/workflows/*.yml
- android/build-linux.sh, android/test-linux.sh

**实际行为**:
- Windows 测试独立 workflow
- Android smoke 独立 workflow
- Release workflow 治理

---

### PR #44 - 可选操场跑道模式（DemaciaForge）
**Head SHA**: cd05b7b
**状态**: OPEN, MERGEABLE ✓
**目标**: 新增 opt-in 标准操场 400m 跑道模式

**修改文件** (8个):
- 新增：src/track/stadium.rs
- 核心：api/flow.rs, track/generator.rs, cli/cmds.rs, ui/run.rs

**实际行为**:
- 新增 `RouteMode::Stadium`
- 400m 标准跑道，8条赛道可选
- CLI: `--stadium` flag
- GUI: 路线配置页选项

---

### PR #51 - 道路模式必经点强制经过（laotierocket）
**Head SHA**: 41694e3
**状态**: OPEN, MERGEABLE ✓
**作者**: laotierocket (他人)
**目标**: 道路模式强制经过必经点 + 随机 N 个普通点

**修改文件** (14个):
- 核心：track/generate_road.rs, api/flow.rs, api/policy.rs
- route-planner: graph.rs, lib.rs
- UI/CLI: ui/run.rs, cli/cmds.rs

**实际行为**:
- 硬点 = 全部必经点 (isFixed!=0) + 随机 `ordinary_pick` 个普通点
- 路径规划强制经过硬点
- CLI: `--ordinary-pick N`
- GUI: 配置页滑块

**与其他 PR 关系**:
- 与 #60 重叠：都修改 generate_road.rs, api/flow.rs, api/policy.rs
- 与 #53 重叠：都修改 track/wire.rs 五点提交逻辑

---

### PR #52 - 响应式水印（DemaciaForge）
**Head SHA**: ba507dd
**状态**: OPEN, UNKNOWN
**目标**: 水印根据窗口大小自动调整位置和透明度

**修改文件** (2个): ui/mod.rs, ui/watermark.rs

**实际行为**:
- 窗口宽度 < 600px 时水印透明度降低
- 窗口缩放时水印位置自适应

---

### PR #53 - 可选绿色电子围栏（DemaciaForge）
**Head SHA**: 4b462ed
**状态**: OPEN, UNKNOWN
**目标**: 围栏不是提交必要条件，按需调用独立接口

**修改文件** (10个):
- 新增：src/api/fence.rs
- 核心：api/flow.rs, api/policy.rs, track/wire.rs, track/generator.rs

**实际行为**:
- 提交前按需调用独立围栏接口
- 围栏空/失败 → 继续自由跑
- 从围栏对象回填 runAreaId
- 提交和 OBS 使用同一份围栏元数据
- 五点 isFixed=1 标记

**与其他 PR 关系**:
- 与 #42 目标相似但实现不同
- #42: 让围栏可选
- #53: 新增独立围栏接口调用
- 两者可能互补或冲突

---

### PR #57 - 跨平台定位/地图/路网（LightWhite520）
**Head SHA**: ba2018f
**状态**: OPEN, MERGEABLE ✓
**作者**: LightWhite520 (他人)
**目标**: GUI 系统定位、地图选点、城市识别、本地/在线路网

**修改文件** (20+个):
- Android: AndroidManifest.xml, MainActivity.java, 新增 SystemLocation.java
- route-planner: load.rs
- UI: 新增 ui/map.rs, ui/osm.rs, ui/jobs.rs
- 核心：api/flow.rs, api/model.rs

**实际行为**:
- 系统定位：Android JNI 获取 GPS
- 地图选点：OSM 瓦片地图 + 点击选择
- 城市识别：经纬度 → 城市名
- 路网：本地 OSM PBF + 在线 Overpass

**与其他 PR 关系**:
- 与 #63 重叠：都新增地图界面
- 与 #66 重叠：都修改 route-planner/load.rs

---

### PR #60 - road 有效里程与完成标记（DemaciaForge）
**Head SHA**: 299b5b9
**状态**: OPEN, MERGEABLE ✓
**目标**: 区分有效里程/总里程，补齐打卡完成标记

**修改文件** (14个):
- 核心：api/flow.rs, api/policy.rs, track/generate_road.rs, track/generator.rs
- UI/CLI: ui/run.rs, cli/cmds.rs

**实际行为**:
- 从 policy speedTop/speedBottom/unqualifiedRate 计算有效配速窗口
- road 模式区分 validDistance/validTime vs totalDistance/totalTime
- 提交使用实际有效累计值
- 慢速无效段保留灰色标记
- 五点 isFixed=1 显式标记

**与其他 PR 关系**:
- 与 #41 重叠：都修改 track/generator.rs, api/flow.rs
- 与 #51 重叠：都修改 generate_road.rs, api/policy.rs
- 与 #53 重叠：都修改 track/wire.rs 五点标记

---

### PR #63 - 手绘路线/高德/轨迹导入（Autumn984）
**Head SHA**: 2544735
**状态**: OPEN, MERGEABLE ✓
**作者**: Autumn984 (他人)
**目标**: 手绘路线模式 + 高德自定义路线 + 轨迹导入

**修改文件** (18个):
- 新增：src/track/custom.rs, src/amap.rs, src/track/geom.rs
- 新增 UI：ui/draw.rs, ui/map.rs, ui/osm.rs
- 核心：api/flow.rs, track/generate_road.rs, track/generator.rs

**实际行为**:
- 手绘模式：全屏高德瓦片底图，点击标定点，Catmull-Rom 拟合平滑曲线
- 高德路线：调用高德步行路径 API
- 轨迹导入：CSV (ECEF → WGS84 → GCJ-02)
- 新增 `RouteMode::Drawn` 和 `RouteMode::Amap`
- 保留配速曲线、哨兵点、打卡点吸附等机制

**与其他 PR 关系**:
- 与 #57 严重冲突：都新增地图界面 (ui/map.rs, ui/osm.rs)
- 与 #44 相似：都新增 RouteMode
- 与 #60, #51 重叠：都修改 generate_road.rs, api/flow.rs

---

### PR #66 - leisure=track 跑道路网接入（jie6886）
**Head SHA**: d385816
**状态**: OPEN, MERGEABLE ✓
**作者**: jie6886 (他人)
**目标**: 支持 OSM leisure=track 跑道进入路网，缝合孤立连通块

**修改文件** (1个): crates/route-planner/src/load.rs

**实际行为**:
- 识别 OSM leisure=track 标签
- 将跑道接入路网图
- 缝合孤立连通块

**与其他 PR 关系**:
- 与 #57 冲突：都修改 route-planner/load.rs

---

## 冲突与依赖关系矩阵

### 文件级冲突
| 文件 | PRs |
|------|-----|
| api/flow.rs | #41, #44, #51, #53, #57, #60, #63 (7个) |
| track/generator.rs | #41, #44, #51, #53, #60, #63 (6个) |
| track/wire.rs | #41, #42, #51, #53, #60 (5个) |
| cli/cmds.rs | #41, #44, #51, #60, #63 (5个) |
| ui/run.rs | #44, #51, #53, #60, #63 (5个) |
| track/generate_road.rs | #41, #51, #60, #63 (4个) |
| api/policy.rs | #41, #51, #53, #60 (4个) |
| route-planner/load.rs | #57, #66 (2个) |
| ui/map.rs, ui/osm.rs | #57, #63 (2个，严重冲突) |

### 功能重叠分析

#### 围栏处理：#42 vs #53
- **#42**: 让围栏元数据可选，验证存在时的格式
- **#53**: 新增独立围栏接口调用，围栏空/失败时继续
- **关系**: 互补，#53 包含 #42 的需求但实现更完整
- **建议**: 采用 #53，它包含独立接口调用

#### RouteMode 扩展：#44 vs #63
- **#44**: 新增 Stadium 模式（400m 跑道）
- **#63**: 新增 Drawn 和 Amap 模式（手绘/高德）
- **关系**: 互补，不冲突
- **建议**: 都采用

#### 地图界面：#57 vs #63
- **#57**: 系统定位 + OSM 地图选点 + 路网加载
- **#63**: 高德瓦片地图 + 手绘路线
- **关系**: 严重冲突，都新增 ui/map.rs 和 ui/osm.rs
- **需要**: 详细比较实现质量和功能覆盖

#### 道路模式：#51 vs #60
- **#51**: 必经点 + 随机普通点强制经过
- **#60**: 有效里程区分 + 打卡完成标记
- **关系**: 互补，但都修改 generate_road.rs
- **建议**: 都采用，需要仔细合并

#### 路网加载：#57 vs #66
- **#57**: 本地 OSM PBF + 在线 Overpass
- **#66**: leisure=track 跑道接入路网
- **关系**: 互补，#66 是 #57 的增强
- **建议**: 都采用

---

## 下一步

1. 详细审查 #57 vs #63 的地图实现（最大冲突点）
2. 审查每个 PR 的代码质量和测试覆盖
3. 确认 bug 和风险
4. 制定整合顺序
