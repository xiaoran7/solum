# Android Release QA

记录包版本、commit、构建类型、ABI、设备型号、Android/API、时间与证据位置。不要用 Debug、模拟器或主机 API 结果替代 Release 真机。

## 门禁

1. `cargo test --workspace`、clippy `-D warnings`、fmt check 全绿。
2. Release APK/AAB 从干净 target 构建，校验签名、版本 `0.2.8/1018`（发布时再读配置）和产物摘要。
3. 干净安装与保留数据升级各走一次；确认 guest/账号 profile 不串数据。
4. 验证注册/登录/refresh/退出、离线 ingest、同步/备份恢复和 Guard。
5. 验证通知权限、监听白名单、精确闹钟、锁屏/进程回收/重启后的提醒重建；不得用 force-stop 作为正常后台测试。
6. 验证断网、TLS、429、超时、服务恢复和错误文案。
7. Health Connect 只在用户明确操作后授权，测试不读取或记录真实健康明细。

发现生产 APK 缺 receiver/service、签名不一致或升级可能丢数据时停止发布，先保全数据和产物证据。
