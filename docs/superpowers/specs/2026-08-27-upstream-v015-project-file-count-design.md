# ReHome 0.1.15 合并与项目文件计数设计

## 目标

在保留现有共享用户 Skills 迁移、schema v1/v2、目录事务和 lock 合并能力的前提下，将上游 `desktop-v0.1.15` 合入 `codex/agents-skills-migration`，并让发送页显示每个已登记项目当前可迁移的真实文件数量。

## 上游合并

固定合并上游 `desktop-v0.1.15@5cb4621`，不移动已经发布的 `agents-skills-preview-v0.1.11` tag，也不重写功能分支历史。

`package.rs` 同时保留共享 Skills schema v2 逻辑和上游的大包兼容：100,000 个 archive entries、16 GiB planning payload 上限、8 次稳定读取重试，以及一次打开并复用的 `AuthenticatedPayloadArchive`。`restore.rs` 的普通文件与 Skill bundle 都通过同一个已认证 archive 写入，Skill lock 继续使用已认证 planning payload，目录隔离、原子替换、journal 和故障注入语义不变。

## 文件计数

计数口径是会进入 `.rehome` 项目 payload 的普通文件数，而不是目录内全部文件数。扫描器只读取目录项和元数据，使用与项目打包相同的 `WalkDir`、`is_forbidden`、路径正规化、symlink 和特殊文件规则；它不复制、不哈希文件。

前端只发送 discovery 返回的 `project_id`。后端重新 discovery，在服务器持有的 inventory 中解析项目路径，不接受 renderer 提供的任意路径。重复 ID 作为无效请求拒绝；未知、缺失、不可用或读取失败的项目返回该项目自己的失败结果，不中断其他项目计数。

## 界面与生命周期

App 在初次 inventory 成功后发起一次后台批量计数，并在 App 层缓存结果。切换首页和发送页不会重复扫描。发送页按项目显示：

- 等待结果：`正在统计文件…`
- 成功：`N 个文件`，包括明确的 `0 个文件`
- 失败：`文件统计失败`
- 项目目录缺失：继续显示 `项目文件缺失`
- 未归属项目的对话：不显示文件状态

计数失败不改变项目原有选择状态，也不影响对话单独迁移。打包时仍按现有安全流程重新扫描，因此列表数字是快照，最终以包报告为准。

## 兼容与交付边界

本次不改变 `.rehome` schema、`ProjectEntry` 序列化格式或应用版本号。本轮只在本地完成文档、上游 merge、功能提交和 macOS 验证；不 push、不创建 tag/Release/PR，也不宣称合并后的 Windows CI 或真实 Mac → Win11 恢复已经验证。
