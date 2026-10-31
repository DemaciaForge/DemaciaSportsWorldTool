# 计分跑抓包核验与修正

核验日期：2026-10-31。依据为用户提供的 2026-10-26 Android 7.4.0 历史抓包及解码文件。
本次仅离线读取本地文件、修改代码和运行测试，没有登录、回放请求或向业务平台提交记录。

## 输入与可复现性

本地原件包括 `计分跑数据.har`、`har_normalized.har`、`cos_run_data_raw.json`、
`cos_run_data_decompressed.json`、`fields_decoded.json` 及 `抓包分析报告.md`。
用户给出的 `cos_run_data_raw\.json` 实际对应同目录的 `cos_run_data_raw.json`。
这些文件保持原样，不将抓包、账号、坐标或签名 URL 放入仓库。

`tools/audit_run_capture.py` 用 Python 标准库完成离线核验：

```powershell
python tools/audit_run_capture.py `
  --obs 'D:\running\cos_run_data_raw.json' `
  --har "$env:USERPROFILE\Downloads\计分跑数据.har" `
  --normalized-har 'D:\running\har_normalized.har' `
  --run-data 'D:\running\cos_run_data_decompressed.json' `
  --fields 'D:\running\fields_decoded.json' `
  --out '.local\har-audit-20261031.json'
```

报告仅包含 SHA-256、白名单字段名及汇总量，不输出头部值、身份、坐标、记录标识或 URL。
`--candidate` 可额外分析一份本地编码 OBS 对象；它只生成独立摘要，不把单条历史记录当成验收模板。
单文件输入上限 32 MiB，单字段解压上限 8 MiB；拒绝重复 JSON 键及非有限数值。
`--out` 不可覆盖任何输入，包括输入的硬链接。

退出码：`0` 表示已提供文件之间的一致性检查通过；`2` 表示交叉检查不一致；`1` 表示输入无法解析。
**退出码 0 不代表跑步达标、轨迹无异常或服务端接受。** 汇总中的 observations 独立列出读数差异。

## CONFIRMED：从原始文件重算的事实

| 核验项 | 结果 |
|---|---|
| HAR 请求数 | 原件及规范化文件均 160 条 |
| HAR 规范化差异 | 仅第 69 条记录的 `response.content.mimeType` 改动；其余内容一致 |
| COS PUT 与 raw JSON | 1 条 PUT，内容与 raw 对象一致 |
| 解码产物 | `run_data` 与解压文件一致；`fields_decoded.json` 全部已提供字段与 gzip 解码文本一致 |
| OBS 字段 | 11 个，每个值为 gzip + Base64；`extension_json`、`segment_json` 解码为空串 |
| 轨迹 | 103 点，每点 27 个字段；`gLat/gLng` 有效，`lat/lng` 为 -1 占位 |
| 计时 | `totalTime` 跨度 427 s；`gainTime` 首末差 505 s、最大最小差 508 s |
| 墙钟采样 | 97 个 5 s 间隔，另有 -3、0、4、8、11 s 各 1 个 |
| `totalTime` 差值 | 84 个 5 s 间隔，14 个 0 s 间隔，另有 1、2、3 s 间隔 |
| 速度/步频窗 | 各 42 个，配对边界一致；窗口宽度 9.957–66.005 s |
| 距离 | 轨迹及圈距合计 1051.801718 m；速度窗合计 1037.591301 m，相差 14.210418 m |
| 步数 | 步频窗合计 848；圈步数合计 860，不能称为完全守恒 |
| 人脸条目 | 2 条历史核验数据；不能复制成新记录的核验结果 |
| 请求信封 | 6 个可观察信封的 `k` 均解码为 128 B，`h` 长度 32 字符 |
| 响应信封 | 15 个可观察信封的 `s` 均解码为 128 B |

这修正了原报告中的几处结论：

- `k=96 B` 和 `s=64 B` 不符合原文件；不能根据 Base64 文本长度猜 RSA 规格、SHA 拼接或签名算法。
- 505 s 是按数组首末 `gainTime` 算的墙钟时长；不能代替 `totalTime=427 s`，也不能把两个时钟的间隔混用。
- 窗口并非始终 5–10 s，圈步数与窗步数也不一致。因此“整条数据完全自洽、无异常”超出了证据。
- 本样本 `isFixed=0` 与 `isPass=true` 同时出现，二者是分类与通过状态，不能互相替代。

## INFERRED：坐标解释

以项目已有 BD-09→GCJ-02 转换计算，5 个点位 `lat/lon` 转换后与各自 `glat/glon`
的距离残差为 **0.038–0.138 m**。这强烈支持点位的两组字段分别是 **BD-09、GCJ-02**，
而非原报告标注的 WGS-84、GCJ-02。

使用 `glat/glon` 与轨迹 `gLat/gLng` 比较，5 个点到最近已记录 GPS 点的距离为
**14.684、3.432、11.919、4.244、18.376 m**；不能把到轨迹中心的距离当成点位通过距离。
最近采样点距离只是几何读数，不能单独确定服务端的通过半径或判定。

## 已落实的代码修正

- `wire.rs`：OBS 补齐 `extension_json=gzip(空串)`；现有扩展及人脸空值不生成虚构核验信息。
- 10 s 窗：对累计步数边界先取整再相减，消除逐窗独立取整的累计误差。
  例如 25 s 累计 4 步，应分配 `[2,1,1]`，旧算法会得到合计 5 步。
- OBS 路径：上传与历史回读共用 UTC+8 小时桶，非法时间报错，不生成空分区。
- `records.rs`：只有详情显式提供有效的 `complete`、`isCheat` 才产生达标结论；缺失字段显示未知。
- CLI、运行结果及详情页面：分别显示上传完成数、详情获取状态及业务判定，消除固定 `/2` 和“获取即通过”。
  运行 CLI 的 `0` 要求上传全部成功且服务端明确达标无异常；`2` 表示记录已保存但后续验证未完成，
  **应查询已有记录，不应自动重提**。`--dry-run` 的 `0` 仍仅表示本地校验通过。
- COS 日志：成功日志去掉签名查询串；失败日志不回显可能带签名 URL 的传输异常或服务端正文。
- 测试：11 字段及 27 点字段的脱敏 schema 固化到 `src/track/fixtures/obs_schema_740.json`；
  增加取整守恒、时区跨日、未知/异常判定、签名日志和离线审计回归。

## 保留现有逻辑与 UNKNOWN

本次没有将固定 10 s 切窗改成样本里的长窗，也没有据单条 `segment_json` 空值删除已有分段逻辑。
样本中的步长哨兵值、窗口 ID、队列号和暂停行为仍属于客户端/场景差异，不能推广为所有版本规则。
项目已有 BD-09→GCJ-02 转换保持不变；Android/iOS 请求版本和加密算法也没有据报告猜测替换。

本样本小时分区与 UTC+8 起跑小时吻合；跨小时跑步时服务端究竟按起跑还是上传小时取桶，仍需额外证据。
本次保留原项目按起跑时刻取桶的约定，仅消除运行机器时区造成的偏移。

抓包内业务响应仍是加密信封，HTTP 200 不能证明 `complete=true`、`isCheat=0`。
实时服务接受、实际计分、风控原因以及其他学校/版本的兼容性均为 **UNKNOWN**。
本地测试证明代码与离线契约，不代表平台验收。

## 回归命令

```powershell
cargo test --all --locked --offline
cargo test --no-default-features --lib --locked --offline
python -m unittest discover -s tools -p test_audit_run_capture.py -v
```

DemaciaSportsWorldTool · As of 2026-10-31
