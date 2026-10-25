# 跑步记录真实性：代码审计与改造依据

> 依据：`running（备份）/../research/elf/r4/ANALYSIS_ANTICHEAT.md`（服务端判定链）、
> `RESULT_MY_SCHOOL_RULES.md`（实测下发阈值）、`RESULT_THRESHOLD_UNITS.md`（配速量纲）、
> `research/research-runner`（`REGULAR_LOOP` / `SPEED_SPIKE` / `TIME_GAP` / `PAUSE_CONFLICT` 四条几何与时序规则）。
> 本文只记录**代码事实**与**改造点**，数值都是代码里的现状，不引用任何服务端实现。

---

## 1. 服务端能做的物理互校（改造的总目标）

一件事先钉死：客户端上报的四组量必须**互相自洽**，任何一组单独看都"像"，合起来对不上就是漏洞。

```
速度(m/s) = 步频(步/分) / 60 × 步幅(m)
速度(m/s) = 相邻点位移 / 相邻点时间差
距离累计  = 逐点位移之和
步频累计  = 逐点步频积分（步数/时间）
```

外加三条几何/时序约束：

| 约束 | 含义 | 代码现状 |
|---|---|---|
| 圆拟合度 | 完美圆 + 半径离散极小 + 转角均匀 = 机器生成 | 经典模式**必然**触发（见 §2.1） |
| 单段瞬时速度上限 | 相邻两点位移/时间差不能远超人类极限 | 吸附与断崖可造出尖峰（§2.3/§2.4） |
| 采样间隔上限 | 相邻点时间差不能超出设备采样周期 | 断崖点时间差可 >60s（§2.4） |

## 2. 逐项审计

### 2.1 经典模式：完美圆，是最强的机器指纹

`src/track/geom.rs::make_point_ring` 用打卡点的质心做中心，把点按极角排序后做
Catmull-Rom 闭合样条；`src/track/generator.rs` 把每个采样点**精确**放在这条弧长环上
（位移累积 `s += direction * d_step`，再用 `ring_point_at` 取点）。

后果：

1. `color_mode = FullGreen` 时抖动硬编码为 0（`jx = 0.0; jy = 0.0`），坐标是**解析精确值**；
2. 环首尾闭合，跑满一圈以上时第二次经过的点与第一次几乎重合；
3. `research-runner` 的 `REGULAR_LOOP`（圆拟合度 > 0.999 且径向离散 < 0.005 且转角均匀 > 0.99）
   在这条路径上稳定成立。

真人绕操场跑也接近圆，但**不是这么圆**：步道宽度、进出操场、让路、不同圈配速都会把
径向离散推到米级。

### 2.2 模式 C（自定义路线）步频绑定的是**平均速度**，不是逐点速度

`src/track/custom.rs:294-300`：

```rust
let v_now = dist / dur_f;                     // ← 全程平均速度，恒定
let stride_target = 0.62 + 0.17 * v_now + 0.03 * sin(...) + gauss(0, 0.008);
let v_cad = v_now * (1.0 + 0.03 * sin(...));
let cad = (v_cad / stride_target * 60.0).clamp(100.0, 200.0);
steps_acc += cad / 60.0 * dt;
```

`w[i]`（逐点速度）虽然算出来了，却**没有参与步频计算**。于是：

- 速度剖面里的热身 ramp、疲劳下降、正弦起伏全部不在步频上体现；
- 用 10s 窗口把 `speed_json` 与 `step_freq_json` 相除，得到的"每步距离"是一串被常数
  拉平的数，与速度序列的相关系数接近 0 —— 正是 `ANALYSIS_ANTICHEAT.md` §3.1 点名的
  "轨迹速度与步频完全解耦"。

同一份代码里的 `generate_road.rs:388-396` 已经是正确写法（`gait(speeds[i], ...)` 再用
`ou_cadence_series`），模式 C 是唯一遗漏的。

### 2.3 点位吸附制造零时长空间跳跃

`generator.rs:498-515`（模式 C 同型逻辑在 `custom.rs:402-420`）：

```rust
if best_d < 40.0 * 40.0 {
    locs[i].gLat = round_to(pl.0, 7);
    locs[i].gLng = round_to(pl.1, 7);
}
```

只改坐标，不改时间。若最近点距离打卡点 39 m，而这个点的 `dt` 只有 1–3 s，
相邻段瞬时速度就是 13–39 m/s（47–140 km/h）。现在能"通过"是因为 FastGreen 模式把
`state` 全写成 1，客户端预判不再拦；服务端按位移/时间差算的是另一回事。

### 2.4 结尾断崖制造 `TIME_GAP`

`src/track/postfix.rs:10-30` 把最后正常点的 `speed` 压到 2.0–2.6 m/s 并改写 `avgSpeed`，
但注释明写"totalDis 保持"。同时采样网格的最后一段是
`dts.push(max(1.0, dur - t_last))`（`generator.rs:111`）。

- 若最后一个正常点恰好是倒数第二个点，最后一段位移很大而时间只有 1 s → 尖峰；
- 若网格尾部正好落在整秒，这段 `dt` 可以是几十秒 → `TIME_GAP`。

### 2.5 `speed` 与位移/时间差无关联

三种模式都按"先有位移，再给 `speed` 加高斯噪声"生成瞬时速度：

```rust
let kmh = avg * 3.6;                 // avg = 本段位移/本段时长
let gps = (kmh + gauss(0, kmh * 0.08)).max(0.0);   // 20% 概率置 0
```

`avgSpeed` 与 `speed` 是两套数，互不约束；异常点（`type = -1`）更是取 `uniform(0.5, 6.0)`
（模式 C 还有 12% 概率 `uniform(15, 46)`）。真人设备的 `speed` 是同一段位移除以同一段
时间差，只差测量噪声。

### 2.6 `totalAscent` 稳定落在 0 附近

`generator.rs:256`：

```rust
let alt_sigma = rng.uniform(3.8, 6.2) / (0.40 * n_est as f64);   // n_est ≈ dur/5
```

`n_est` 等于采样点数，所以 σ ∝ 1/N，而 `track::altitude::ASCENT_NOISE_THRESHOLD_M`
恒为 0.15 m。真实记录里 `laps_json.elevationGain` 与顶层 `totalAscent` 几乎总是 0，
是"数据被压平"的直接读数。

### 2.7 10s / 60s 窗口只除以时间

`generator.rs:385-399` 与 `submit.rs::android_tensec` 都用"本窗距离 ÷ 本窗秒数"，
不处理边界插值。因此：

- 窗口不会跨暂停（暂停不存在），但**断崖点之后**的窗口距离为 0 而时间仍计满；
- 模式 C 的异常点（`type = -1`）位移不计入 `dist_acc`，却计入 `t_acc` 与步数，
  对应窗口出现"有步数、零距离"。

### 2.8 轨迹语义：每次都是一条新路

`geom.rs` 的环首尾相接，但缺少"从哪进、停在哪、跑几圈"的语义。真人一次跑步的形态是：

1. 从停车点走到起点（约 50–150 s，约 100–200 m）；
2. 沿同一条跑道绕 N 圈（跑道是**跑道**，不是一个随机闭合环）；
3. 跑完走出跑道回到停车点。

## 3. 改造清单

| # | 文件 | 改动 | 针对的读数 |
|---|---|---|---|
| A | `track/generator.rs` | 逐点速度剖面在"缓慢漂移的最近点"上取值，不再落在解析环上 | 圆拟合度、径向离散 |
| B | `track/generator.rs` | 加"入口段 + 绕圈 + 收尾段"状态机 | 轨迹形态、圈数一致性 |
| C | `track/postfix.rs`、`generator.rs`、`custom.rs` | 吸附改为带时间成本的坐标修正 | 尖峰 |
| D | `track/postfix.rs` | 断崖点重算 `totalDis`/`totalTime` 并夹住最后一段 `dt` | `TIME_GAP`、尖峰 |
| E | `track/custom.rs` | 步频/步幅改为绑定 `speeds[i]`（复用 `route_planner` 生物力学） | 速度↔步频解耦 |
| F | 三种模式 | `speed` 改为"本段位移 / 本段实测时间差 + 测量噪声" | 峰值速度与轨迹不符 |
| G | 新模式 `track/realism.rs` | 物理互校自检：速度/步频/步幅/距离/步数/上升/心跳 | 全部 |
| H | `track/realism.rs` | 校验通过才允许上报，dry-run 打印逐窗读数 | 可观测性 |
| I | `track/altitude.rs` | 海拔噪声改为每米计，冷启动段与 pace 同相 | `totalAscent` |
| J | `track/wire.rs`、`submit.rs` | 10s/60s 窗口按时间边界线性插值 | 窗口计数不守恒 |

### 3.1 第二轮：载荷字段级

第一轮把「四组量之间的关系」修好了，但载荷本身还有几处**字段级**破绽——它们与
物理无关，只要服务端把这份记录和真机记录并排放就能看出来。

| # | 位置 | 破绽 | 改法 |
|---|---|---|---|
| K | `submit.rs` / `wire.rs` | 提交体与 OBS 各自算一遍 10s 窗：OBS 用插值，提交体取「边界前最后一个采样点」。采样点不落在 10 秒边界上时两边读数不同 | 窗口只由 `wire::ten_second_spans` 定义一次，两处都读它 |
| L | `wire.rs` | `stepDistance` 恒为 `0.0`。官方点结构里它是**累计步幅距离**（`OooO00o.java` 用两点相减得到窗口值），跑 2 km 记 0 米 | 改为 `Σ(本段步数 × 全局步幅估计)`，末点 ≈ `totalDis` 且逐点缓慢漂移 |
| M | `generator.rs` | `speed` 与「本段位移/时间差」的比值几乎恒为 1（噪声 σ=5%），且从不掉零 | σ=18%；约 19% 的点读数为 `0.0`（芯片给不出速度）；保留 2% 遮挡掉值 |
| N | `realism.rs` | 采样间隔最短 3 s，清一色 5 s 为主 | 恢复 1–8 s 真实分布（实测 918 s 记录里 1 s 出现 7 次、2 s 出现 6 次） |
| O | `wire.rs` | 点结构缺 `accuracy` / `hasAltitude` / `steps` | 与官方点结构对齐，补齐后共 30 键 |
| P | `generator.rs` | `radius`、`count` 逐点独立随机；真机是缓慢游走（相邻点几乎相同） | 改为围绕会话基准的 OU 游走 |
| Q | `api/flow.rs` | 真实性自检只挂在 dry-run，正式提交没有闸门 | `run_full_flow` 提交前 `audit`，不通过则中止并打印逐项读数 |
| R | `realism.rs` | 只校验 `track.speedPerTenSec`（内部序列），而真正发出去的是另一份 | 新增「窗时长合计 = 总时长」「逐窗配速按实际窗宽」「途中无停顿窗」三项，校验对象就是 `ten_second_spans` |

## 4. 判定口径

A–J 全部落地后，同一条记录同时满足：

1. `v_speed_df = Σ(位移DT·Δt) / Σ(位移·Δt)` 落在 `[0, 0.10]`，且 `v_pace_df` 落在 `[0, 0.10]`；
2. `|位移距离 / totalDis − 1| ≤ 0.02`；
3. `|步数 / totalSteps − 1| ≤ 0.02`；
4. 瞬时步长均值落在 `[0.50, 1.70]` m，标准差 `≤ 0.22` m；
5. 相邻段瞬时速度峰值 `≤ SPEED_CEIL × 1.35`；
6. 相邻点 `dt ≤ 30 s`（采样周期上限）；
7. 圆拟合度 `< 0.999`；
8. 10s 窗距离之和 / 60s 段距离之和 与总距离误差 `≤ 3%`；
9. 里程 ≥ 1 km 时 `totalAscent > 0`；
10. `laps_json` 每圈 `avgPace` 之和与总时长相符，`avgCadence` 与 `avgStride` 乘积回到速度。

K–R 落地后，另外满足：

11. 提交体与 OBS 的 10s 窗逐项相等，且窗时长合计 `= totalTime`；
12. `stepDistance` 末点与 `totalDis` 相差 `≤ 10%`，且逐点单调不减；
13. `speed == 0` 的点占比落在 `5%–35%`（实测 19%）；
14. `speed / (位移/时间差 × 3.6)` 中位比落在 `0.85–1.15`，且 p90−p10 `> 0.35`（不能"过于完美"）；
15. 采样间隔 5 s 占比 `55%–92%`，且至少出现一次 `≤ 2 s` 的加密采样；
16. 途中（首末窗除外）不存在配速低于 `1.0 m/s` 的 10s 窗。

这些阈值都写在 `track::realism` / `track::wire` 的常量与测试里，改动即回归。
