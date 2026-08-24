# Android 发布与提醒 QA

本页只记录可重复、不会借测试破坏用户数据的验收配方。真机未执行的步骤必须继续标为“待验证”。

## 1. 核对已签名 Release APK

从仓库根运行：

    node scripts/verify-android-release.mjs D:\path\to\app-arm64-release.apk

脚本只读 APK，不安装、不重签名、不修改产物。它会核对：

- <code>tauri.conf.json</code> 版本与 Cargo workspace 版本一致，APK 的 <code>versionName/versionCode</code> 又与源码一致；
- aapt badging 中的包名、minSdk、targetSdk 与 ABI；
- ZIP 中每个声明 ABI 的 <code>libsolum_app_lib.so</code>、资源表、manifest 与 Tauri 配置；
- 合并 manifest 同时含 <code>AlarmReceiver</code>、<code>BootReceiver</code>、<code>SolumNotificationListener</code>；
- apksigner 的 v2 签名、签名证书摘要、APK 字节数与 SHA-256。

成功后末尾会输出可直接粘贴到 CHANGELOG 的证据行。多个 APK 并存时必须逐个显式传路径，脚本不会猜“最新”文件。

## 2. 后台进程回收后提醒

只在 Debug 包中执行：

1. 打开“设置”，确认“提醒可靠性”显示后台接收与重启恢复已就绪；若有红色告警，停止测试并保留截图/logcat。
2. 新建一条唯一命名、2–5 分钟后到点的合成提醒，确认系统已注册对应 AlarmManager 项。
3. 回到“设置 → 提醒可靠性”，点“模拟后台回收”，阅读说明后确认。该入口只杀当前进程，不清数据、不改配置，也不取消系统持有的 PendingIntent。
4. 用 <code>adb shell pidof dev.solum.app</code> 确认旧进程已退出；不要使用 <code>am force-stop</code> 或系统设置里的“强行停止”，它们会主动取消/阻断 alarm，不能模拟普通后台回收。
5. 等待到点通知。记录通知截图、触发时间、<code>dumpsys alarm</code> 和 logcat；重新打开 Solum，确认提醒状态最终收敛且没有重复通知。
6. 只删除本次唯一命名的合成事件，不碰用户原有日程。

Release 包不会展示该按钮，Rust 与 Kotlin 两层都会拒绝进程死亡注入。

## 3. 福利告警通知链路

前提：手机已用 QA 账号登录目标 Solum Center，通知权限与通知处理服务正常；先等待至少 25 秒，让首次轮询只建立该账号的告警游标。使用同一 QA 账号的短期 access token，令牌只放环境变量，不写进文档、截图或命令输出。

    $qaEventId = "qa-benefit-test-$([guid]::NewGuid().ToString('N'))"
    $qaBody = @{
      event_id = $qaEventId
      source = "release-qa"
      monitor_id = "android-notification-chain"
      name = "福利监控通知链路"
      status = "test"
      checked_at = [DateTime]::UtcNow.ToString("o")
    } | ConvertTo-Json -Compress
    $qaRequest = @{
      Method = "Post"
      Uri = "$env:SOLUM_QA_SERVER/v1/alerts"
      Headers = @{ Authorization = "Bearer $env:SOLUM_QA_ACCESS_TOKEN" }
      ContentType = "application/json"
      Body = $qaBody
    }
    Invoke-RestMethod @qaRequest

预期在下一次轮询（通常 4–24 秒）出现标题“福利监控测试”、正文“Solum 通知链路正常”。验收同时检查：

- 服务端返回数值 <code>seq</code>，手机只收到同一租户的新鲜事件；
- 用相同 <code>event_id</code> 重放不会产生第二条通知；
- 事件不携带账户、余额、密钥或任意通知正文，并按服务端 7 天保留期自然清理；
- 若手机是该账号首次轮询，首次只建游标、不补发旧告警；应等初始化完成后再用新的唯一 <code>event_id</code> 重测。
