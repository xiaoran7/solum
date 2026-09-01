# Solum 持久踩坑

## 构建与仓库

- 不搬运 `target/`：Cargo/Tauri 产物含绝对路径和旧权限元数据；迁移工作区后先干净重建。
- Windows 不用 `>` 生成 SQLite/二进制或多行源码；避免产生 `nul`。临时文件放仓库外。
- Android 交叉编译保持 rustls/纯 Rust 依赖，新增 crate 前检查 native C/OpenSSL 链。
- Android 与 HarmonyOS 是独立 Git 仓，不能从父目录操作或跨仓一次提交。

## 数据与同步

- Core 时间必须注入；测试读取系统时钟会造成不可复现的提醒/合并错误。
- SQLite schema、同步表白名单、触发器、apply 分支和固定夹具必须一起演进；少一处会形成静默 history gap。
- JSON 解析失败是错误，不等于“未知表隔离”；隔离只处理被明确识别为可泊车的未来数据。
- 状态文件和导入使用临时文件 + flush + rename；恢复只写入经过完整预检的空目标。
- `local_only` 是数据分类，不是 UI 标签；任何 recall/LLM 路径都必须在数据层过滤。

## Android/设备

- “权限已授予”不等于后台能力工作；receiver/service 必须出现在 Release manifest，并验证锁屏、进程回收与重启。
- `force-stop` 会让 Android 主动阻断 alarm/receiver，不能用于正常后台恢复验收。
- 浏览器健康接口成功不证明应用 rustls 可达；按 DNS、SNI、ClientHello、代理和设备网络逐层探测。
- `offsetParent === null` 对 fixed/sticky 元素不代表不可见；用 computed style、视口矩形和必要的截图判断。
- DOM 属性/类名正确不等于渲染正确；mock-IPC 后仍需真实浏览器/设备走查。
- WebView 瞬态界面必须统一登记到同一个栈顶 history 条目；漏掉一种弹窗/面板会让 Android 返回键在浮层仍可见时误切底层视图。程序关闭浮层后的异步 `popstate` 也要吞掉，不能再触发一次视图回退。
- 响应式覆盖必须声明在对应桌面基础规则之后；账号门曾因桌面双列规则后声明，在 390px 下把说明栏压成单字竖排并覆盖表单。真实 320/390/1000px 渲染是发布门槛，不能只看选择器存在。
- 不要在启动或一次对话写入后刷新所有隐藏页面；台账单次刷新会并发读取多个数据层。隐藏视图进入时再刷新，高频刷新要 single-flight，页面隐藏时暂停周期任务。
- 渲染函数内向 `document` 注册监听器必须可解绑；组件详情菜单曾在每次进入时累积外部点击监听，造成越用越慢和重复回调。
- `account_status_get.logged_in=true` 不代表会话仍可用；刷新 token 失败会写入 `state_error`。只渲染错误文案会把用户困在伪登录态，必须与普通 IPC 的 401/过期错误一起进入统一退出与登录重定向。
- 不把相册原图直接塞进 `localStorage`：高像素照片会迅速撞上 WebView 配额并放大解码内存。先限制输入体积、居中裁切、压缩并再次检查编码后大小；解码失败或超限时保留原头像。

## 安全

- Guard token 一次性且绑定动作/参数/主体；模型或页面不能自行构造。
- 不把生产 token、通知正文、健康数据、设备账号或签名材料写入日志/文档/测试夹具。
- 主机 API、模拟器、Debug 和真机 Release 结论各自独立，报告必须写清证据层级。
