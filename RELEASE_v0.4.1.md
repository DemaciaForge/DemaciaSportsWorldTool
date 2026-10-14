# Release v0.4.1

**发布日期**: 2025-01-26

## 🔒 安全改进

### 日志脱敏优化 (#69)
- **新增**: `redact_json()` 函数 - 智能 JSON 脱敏，保留结构
  - ✅ 只脱敏字符串值（数字、布尔、null 保持原样）
  - ✅ 递归处理嵌套对象和数组
  - ✅ 大小写不敏感的敏感键匹配
- **改进**: HTTP 响应和登录会话日志脱敏
  - `src/api/client.rs` - 统一使用 `redact_json()`
  - `src/api/login.rs` - 统一使用 `redact_json()`
- **测试**: 新增 3 个测试覆盖边界场景
  - `test_redact_json_preserves_other_values` - 类型保留
  - `test_redact_json_nested_objects_and_arrays` - 嵌套结构
  - `test_redact_json_sensitive_keys_case_insensitive` - 大小写

**贡献者**: @Erfan817

## 🎯 功能增强

### 跑道路网集成 (#66)
- **新增**: `leisure=track` 跑道标签支持
- **改进**: 孤立连通块自动缝合
- **优化**: 路网拓扑完整性

### 自定义路线 (#63)
- **新增**: 高德步行路径规划
- **新增**: 绕圈/圈数控制
- **支持**: 自定义起终点

## 📊 测试覆盖

```
✅ 库测试:        135/135 通过
✅ route-planner:  10/10 通过
✅ Release 编译:  成功
```

## 🔧 技术细节

### 合并的 PR
- #69: 日志脱敏 token 与响应敏感字段 (@Erfan817)
- #66: leisure=track 跑道接入路网
- #63: 自定义路线功能

### 重构
- 删除旧的 `normalized_key()` 和 `sensitive_key()` 函数
- 统一使用全局 `SENSITIVE_KEYS` 常量
- 简化脱敏逻辑调用路径

### 已知问题
- `--dry-run` CLI 功能未连接（功能本身已实现）
- 5 个未使用代码警告（不影响正常使用）

## 📦 下载

### Windows (x64)
- `NekoSportsWorldTool-v0.4.1-windows-x64.zip`

### 源代码
- `Source code (zip)`
- `Source code (tar.gz)`

## 🔄 升级说明

从 v0.4.0 升级到 v0.4.1：
1. 下载新版本可执行文件
2. 替换旧版本
3. 配置文件兼容，无需修改

## 🙏 致谢

感谢以下贡献者：
- @Erfan817 - 日志脱敏优化
- @YanamiNeko - 项目维护

---

**完整变更日志**: v0.4.0...v0.4.1
