# Documentation index

更新日期：2026-08-24

## 工作区与仓库边界

本主仓当前位于 `Project/solum/android`，并继续承载共享 core、服务端与权威协议文档；HarmonyOS 独立仓位于兄弟目录 `../harmony`，iOS 目录仅预留。父级 [`Project/solum/`](../../README.md) 是本地平台容器而非 Git 仓库。跨平台修改必须进入各自仓库分别测试和提交，不能把目录相邻误当作同一工作树。

- 当前方向总览：[`MOBILE-ROADMAP.md`](MOBILE-ROADMAP.md)——Mobile-only / Android-first、生产与迁移快照、加密备份、信息源、Agent 质量、移动排版与分阶段路线的快速入口。
- 入门和当前产品：根 [`README.md`](../README.md)、[`PRODUCT.md`](PRODUCT.md)。
- 架构和设计：[`ARCHITECTURE.md`](ARCHITECTURE.md)、[`DESIGN.md`](DESIGN.md)。
- 集成与隐私：[`LLM-PROVIDERS.md`](LLM-PROVIDERS.md)、[`PRIVACY.md`](PRIVACY.md)。
- 当前未完成项：[`TODO.md`](TODO.md)。
- 持续历史（三份大文件，按 AGENTS.md 要求开工前先检索；两份已带主题索引）：
  - [`CHANGELOG.md`](CHANGELOG.md)：功能新增/完善/修改的完整记录。查「某功能什么时候进的 / 某版本改了什么」。头部有**版本速查索引**；建议关键词：功能名、crate 名、schema vN、版本号。
  - [`PITFALLS.md`](PITFALLS.md)：踩坑集（现象/根因/解决/如何避免）。排查前必搜。头部有**主题索引**；建议关键词：Android 构建、量具/断言、迁移、同步、CSP、PowerShell。
  - [`MISC.md`](MISC.md)：被否方案、取舍原因、非功能决定。别重新讨论已拍板的事，先搜这里。头部有**主题索引**；建议关键词：红线、拍板、否决、取舍、命名契约。

当前 Android 源码/发布输入为 0.2.8（versionCode 1018）。APK 元数据已核对，但 vivo 覆盖安装、提醒 E2E 与剩余生产设备矩阵仍待验证；旧 0.2.6 设备证据不能替代新包验收。
