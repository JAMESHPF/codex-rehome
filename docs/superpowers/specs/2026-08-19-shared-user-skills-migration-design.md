# ReHome 共享用户 Skills 迁移设计

## 目标

让 ReHome Desktop 在保持现有 schema v1 兼容性的前提下，发现、打包并恢复用户级 `~/.agents/skills`，同时选择性合并 `skills` CLI v3 lock 元数据。恢复必须以 Skill 目录为原子单位，避免逐文件合并形成混合内容，并纳入现有事务、校验和崩溃恢复体系。

## 边界

- 支持用户级逻辑根 `$HOME/.agents/skills`；不支持项目级或任意额外 roots。
- `$CODEX_HOME/skills` 继续作为旧版来源，但 `.system` 等可重建内容不导出。
- 不迁移登录状态、Cookies、环境变量、密钥、运行时或长期双向同步。
- `.rehome` 为私人迁移包；仓库测试只使用合成 fixtures。

## 路径与发现

Codex Home 与 Agents Home 独立解析。macOS 从 `HOME`、Windows 从 `USERPROFILE` 得到用户目录；`CODEX_HOME` 不影响 Agents 根。lock 优先使用 `XDG_STATE_HOME/skills/.skill-lock.json`，否则使用用户目录下 `.agents/.skill-lock.json`。

扫描器将共享根解析为单一 canonical directory。逻辑根本身可以是 symlink，但 Skill 内部 symlink、reparse point、socket、FIFO、设备文件和越界对象会阻止该 Skill 被选择。旧 Codex 根中的 symlink 只用于识别指向共享 Skill 的别名并去重，不会被归档。`.git`、依赖目录和构建缓存按现有规则排除并计数；敏感文件阻止整个 Skill 打包。

## 包格式

仅选择现有 Codex 内容时继续生成 schema v1。选择共享 Skills 时生成 schema v2，并增加：

- `shared_skills`: 每个 Skill 的相对目录、归档根、文件数、字节数、排除统计和 ReHome tree hash。
- `agents/skills/<relative>/...`: 共享 Skill 普通文件。
- `agents/metadata/skill-lock-v3.json`: 仅包含已选 Skill 的安全 v3 lock 条目。

新版本读取 v1/v2；旧版本因不支持 v2而在规划前拒绝。所有新 payload 都进入现有校验和与 archive path collision 校验。

## 恢复与冲突

共享 Skill 以目录为原子单位分类：目标缺失为新增、tree hash 相同为不变、不同为冲突。冲突默认保留目标，用户可逐项选择迁移包。大小写或 Unicode 归一化后的同名目录视为同一个目标。

使用迁移包时，在目标父目录内暂存并校验新目录；现有目录先复制到事务备份，再同卷移动到事务隔离名，最后把暂存目录移动到正式位置。journal 记录每个阶段，崩溃恢复能够完成或回滚；无关 Skills 不受影响。

lock 合并遵循同一目录决策：新增或迁移包获胜时写入来源条目，保留目标时保留目标条目，内容相同时优先保留已有目标条目。只支持 v3；目标 lock 缺失时创建，未知版本或损坏时保留原文件并跳过合并。`dismissed`、`lastSelectedAgents` 和无关条目保持目标值。

## 验证

验证分为四层：归档校验和、恢复文件/tree hash、Codex discovery、功能抽样。文件一致或被 Codex 发现不等于脚本运行时在 Windows 已安装。真实个人数据只在私人 Mac → Win11 验收中使用，不进入提交或 PR。
