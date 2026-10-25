# PR 审核状态记录

**审核开始时间**: 2026-10-14
**最后更新**: 2026-10-14 14:00

## ✅ 已完成整合并发布

### PR #69: 日志脱敏 token 与响应敏感字段
- **作者**: @Erfan817
- **审核 head SHA**: `7badfe5b9b5d4e1f3c29d85dd2b1ac6a15f0279d`
- **状态**: ✅ 已整合到 fork main (989a857)
- **处理**: 完整采用，已解决合并冲突
- **修复提交**: 56507b8 (合并提交)
- **测试**: 135/135 库测试通过，10/10 route-planner 通过
- **发布**: v0.4.1 (https://github.com/DemaciaForge/DemaciaSportsWorldTool/releases/tag/v0.4.1)
- **构建产物**: DemaciaSportsWorldTool-v0.4.1-windows-x64.tar.gz (4.1 MB)
- **SHA256**: `b28264fdc73944dd424a2f19b5a218da8ccfc07287396a292404adc3beade282`

**实际变化**:
- 新增 `redact_json()` 函数 - 智能 JSON 脱敏，保留类型
- 在 `src/api/client.rs` 和 `src/api/login.rs` 中统一使用
- 删除旧的 `normalized_key()` 和 `sensitive_key()` 函数
- 新增 3 个脱敏测试

**已确认问题**: 无（合并冲突已解决）

---

## 📋 待审核 PR 列表

根据任务要求，需要审核以下 11 个 PR：

### 我的 PR (DemaciaForge)
1. ❓ #41 - fix: 拆分指标修复与已验证整合内容 - **OPEN**
2. ❓ #42 - fix: make fence metadata optional for submissions - **OPEN**
3. ❓ #43 - ci: split Windows tests from Android smoke and govern releases - **OPEN**
4. ❓ #44 - feat: add opt-in athletics track mode - **OPEN**
5. ❓ #52 - fix(ui): make free distribution watermark responsive - **OPEN**
6. ❓ #53 - 修复校园跑可选绿色电子围栏 - **OPEN**
7. ❓ #60 - fix: 保留 road 有效里程语义并补齐打卡完成标记 - **OPEN**

### 其他人的 PR
8. ❓ #51 - feat(track): 道路模式必经点+随机普通点强经过，数量可配置 (@laotierocket) - **OPEN**
9. ❓ #57 - feat: 跨平台 GUI 系统定位、地图选点与城市识别，支持本地及在线路网 (@LightWhite520) - **OPEN**
10. ❓ #63 - feat: 手绘路线模式（地图点选+平滑曲线）+ 高德自定义路线 + 轨迹导入 (@Autumn984) - **OPEN**
11. ❓ #66 - feat(route-planner): 支持 leisure=track 跑道进入路网并缝合孤立连通块 (@jie6886) - **OPEN**

### 额外发现
12. ✅ #69 - fix(api): 日志脱敏 token 与响应敏感字段 (@Erfan817) - **MERGED** (已整合到 v0.4.1)

### 新发现的 OPEN PR (未在原始列表中)
- #70 - fix(template): handle self-closing GPX trackpoints (@Erfan817)
- #71 - fix(safety)!: 跑步默认本地预览并阻止合成轨迹上传 (@Erfan817)
- #72 - fix(ui): preserve data and report refresh failures (@Erfan817)
- #73 - fix(android): correct PowerShell quoting in release script (@Erfan817)
- #74 - fix(ui): ignore stale run and AI detail responses (@Erfan817)
- #75 - fix(cli): preserve option values beginning with a hyphen (@Erfan817)

**注意**: 按任务要求，默认只处理原列表的 11 个 PR。新 PR 需要用户明确指示是否纳入审核范围。

---

## 🔄 下一步行动

1. 重新枚举 GitHub 上的实际 PR 列表
2. 确认每个 PR 的当前状态（open/closed/merged）
3. 逐个审核上述 11 个 PR 的完整 diff
4. 识别 PR 之间的依赖和冲突关系
5. 修复/评论/整合通过审核的 PR
6. 更新此状态文件

