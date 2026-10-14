## 步频步幅计算差异分析

根据代码审查，我发现了**不同模式的步频步幅计算方式存在差异**：

### 经典打卡点环模式（Legacy）

**文件**: `src/track/generator.rs:354-360`

```rust
// 经典模式：简单公式
let stride_target = 0.62 
    + 0.17 * v_now 
    + 0.03 * sin(周期性波动) 
    + 高斯噪声;
let v_cad = v_now * (1.0 + 0.03 * sin(周期性波动));
let cad = (v_cad / stride_target * 60.0).clamp(100.0, 200.0);
```

**特点**:
- 步幅: 0.62 + 0.17 * 速度（基础公式）
- 步频: 由速度除以步幅反算
- 范围: 步频 100-200 步/分

### 道路模式（Road）

**文件**: `src/track/generate_road.rs:388-396` + `crates/route-planner/src/biomech.rs:19-32`

```rust
// 道路模式：生物力学联动
let targets: Vec<f64> = (0..n).map(|i| {
    let km_done = (times[i] / dur_f) * dist / 1000.0;
    gait(speeds[i], fatigue_from_km(km_done)).cadence
}).collect();
let cadences = ou_cadence_series(&targets, &dts, cadence_seed, 0.25, 0.5, 110.0, 220.0);

// gait 函数（biomech.rs）
let target_cad = (150.0 + 12.0 * v) * (1.0 - 0.05 * fatigue);
let cadence = target_cad.clamp(110.0, 220.0);
let stride = v * 60.0 / cadence;
// 步幅越界时反解步频，保持等式成立
if stride > 1.7 { stride = 1.7; cadence = v * 60.0 / stride; }
else if stride < 0.5 { stride = 0.5; cadence = v * 60.0 / stride; }
```

**特点**:
- 步频: 150 + 12 * 速度（考虑疲劳度）
- 步幅: 由公式 `v = (cadence/60) * stride` 严格推导
- 范围: 步频 110-220 步/分，步幅 0.5-1.7 米
- 引入疲劳度: 跑 0.5km 后步频逐渐下降

---

## 问题原因推测

@WMR-Ram 你的问题可能是：

### 1. **使用了道路模式而非经典模式**

如果你选择的是**道路模式（Road）或操场跑道模式（Athletics Track）**，步频步幅计算会使用新的生物力学公式，与经典模式不同。

**表现**:
- 步频更高（110-220 vs 100-200）
- 步幅范围更严格（0.5-1.7m）
- 考虑疲劳度（后半程步频下降）

### 2. **速度设置导致步频过高或步幅异常**

道路模式的公式：
```
步频 = 150 + 12 * 速度（m/s）
```

如果速度设置为 3.5 m/s（12.6 km/h）：
```
步频 = 150 + 12 * 3.5 = 192 步/分
步幅 = 3.5 * 60 / 192 = 1.09 米
```

如果速度设置过高（比如 4.5 m/s）：
```
步频 = 150 + 12 * 4.5 = 204 步/分（接近上限）
步幅 = 4.5 * 60 / 204 = 1.32 米
```

### 3. **经典模式的公式差异**

如果你确实使用的是经典模式，但速度设置为 3.5 m/s：
```
步幅目标 = 0.62 + 0.17 * 3.5 = 1.215 米
步频 ≈ 3.5 * 60 / 1.215 ≈ 173 步/分
```

---

## 验证方法

请提供以下信息：

1. **路线算法模式**：
   - 经典打卡点环（Legacy）
   - 真实道路拓扑路由（Road）
   - 操场跑道（Athletics Track）
   - 自定义路线（Custom）

2. **速度设置**：
   - 平均速度（m/s 或 km/h）
   - 速度范围（最小-最大）

3. **实际数据**：
   - 总里程
   - 总时长
   - 显示的步频
   - 显示的步幅
   - 你期望的正常值

4. **对比测试**：
   - 尝试切换到**经典打卡点环模式**，看问题是否消失

---

## 我的测试结果

**配置**:
- 模式: 经典打卡点环（Legacy）
- 速度: 2.8 m/s (10 km/h)
- 距离: 3.0 km

**结果**:
```
步幅 ≈ 0.62 + 0.17 * 2.8 = 1.096 米
步频 ≈ 2.8 * 60 / 1.096 ≈ 153 步/分
```

**服务端显示**: 正常，无异常

如果你使用的是道路模式或操场模式，步频步幅的计算公式完全不同，这可能是你看到差异的根本原因。

---

## 建议

1. **确认使用的模式**：截图或告知你选择的是哪种模式
2. **如果是道路/操场模式**：这是新的生物力学算法，步频步幅范围不同
3. **如果是经典模式**：提供速度设置，我帮你计算预期值
4. **临时解决方案**：如果不习惯新算法，可以切换回经典打卡点环模式

期待你的回复！

---

## 最终状态 (2025-01-26)

✅ **PR #69 已成功合并到本地 main 分支**

### 合并摘要
- **冲突文件**: 2 个 (src/api/client.rs, src/api/login.rs)
- **解决策略**: 采用 PR #69 的 `redact_json()` 实现
- **删除函数**: `normalized_key()`, `sensitive_key()` (旧 fork 设计)
- **测试结果**: 
  - 库测试: 135/135 通过 ✅
  - route-planner: 10/10 通过 ✅
- **编译结果**: Release 构建成功 ✅
- **提交哈希**: `2cebbe4`

### 采用的设计方案
1. **脱敏函数**: `redact_json()` - 只脱敏字符串值，保留数字/bool/null
2. **敏感键常量**: 使用全局 `SENSITIVE_KEYS` 数组（与上游一致）
3. **测试覆盖**: 
   - `test_redact_json_preserves_other_values` - 类型保留测试
   - `test_redact_json_nested_objects_and_arrays` - 嵌套结构测试
   - `test_redact_json_sensitive_keys_case_insensitive` - 大小写不敏感测试

### Git 历史
```
* 2cebbe4 (HEAD -> main) Merge PR #69: 采用上游 redact_json 实现
* 996e22c (origin/main, origin/HEAD) Merge pull request #69
* 96f6b97 更新 CHANGELOG.md: 记录 v0.4.1 修复
```

### 遗留问题（非阻断）
1. **未使用代码警告** (5 个)：
   - `dry_run` 变量 (src/cli/cmds.rs:86)
   - `DryRunSummary` 结构体
   - `dry_run_points()` 函数
   - `run_dry_run()` 函数
   - `build_road()` 函数
   
   **影响**: CLI 的 `--dry-run` 功能未实际连接，但不影响正常使用

2. **PDB 文件名冲突警告** (Windows 构建系统特有，非代码问题)

### 下一步建议

#### 可选优化 (非紧急)
1. 在 `src/cli/cmds.rs` 中实际使用 `dry_run` 标志调用 `api::flow::run_dry_run()`
2. 考虑是否保留 `build_road()` 函数（如果未来需要）

#### 准备推送
```bash
# 检查提交
git log --oneline -5

# 推送到远程（如果需要）
git push origin main
```

**负责人**: 德布罗煜 <j1379647633@outlook.com>
**状态**: ✅ 已完成并验证