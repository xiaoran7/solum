package dev.solum.notifaccess

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.util.AtomicFile
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.nio.charset.StandardCharsets
import java.time.Instant
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * F20's foreground service keeps the same application process that hosts
 * Tauri/Rust alive. It never parses captured notification content itself; its
 * only network work is polling account-scoped recovery alerts.
 */
class NotificationPipelineService : Service() {
    private val poller = Executors.newSingleThreadScheduledExecutor()

    override fun onCreate() {
        super.onCreate()
        running = true
        ensureChannel()
        startForeground(NOTIFICATION_ID, buildNotification())
        poller.scheduleWithFixedDelay(::pollAlertsSafely, 4, POLL_SECONDS, TimeUnit.SECONDS)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        poller.shutdownNow()
        running = false
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Solum 通知处理",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "保持 Solum 在后台处理你已授权应用的通知"
            setShowBadge(false)
        }
        manager.createNotificationChannel(channel)
        // 2026-07-20 改名决议的残留清理：渠道 ID 曾是 pa_notification_pipeline，
        // 用户可见名当时已改但 ID 没跟。新渠道创建后删除旧渠道，避免设置里
        // 留下一条永远不再使用的"已删除"通道。
        manager.deleteNotificationChannel(LEGACY_CHANNEL_ID)
    }

    private fun buildNotification(): Notification = NotificationCompat.Builder(this, CHANNEL_ID)
        .setSmallIcon(android.R.drawable.ic_dialog_info)
        .setContentTitle("Solum 正在处理已授权通知")
        .setContentText("重要通知即时处理，其余按设置定时整理")
        .setOngoing(true)
        .setCategory(NotificationCompat.CATEGORY_SERVICE)
        .build()

    private fun pollAlertsSafely() {
        try {
            pollAlerts()
        } catch (_: Exception) {
            // The service is deliberately quiet while offline or unconfigured.
            // It retries on the next interval; the foreground notification remains.
        }
    }

    private fun pollAlerts() {
        val config = loadSyncConfig() ?: return
        val prefs = getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val cursorKey = "$KEY_CURSOR:${config.cursorScope}"
        val initializedKey = "$KEY_INITIALIZED:${config.cursorScope}"
        val since = prefs.getLong(cursorKey, 0L)
        val initialized = prefs.getBoolean(initializedKey, false)
        val endpoint = "${config.url.trimEnd('/')}/v1/alerts?since=$since&limit=60"
        val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"
            connectTimeout = 10_000
            readTimeout = 10_000
            setRequestProperty("Accept", "application/json")
            setRequestProperty("Authorization", "Bearer ${config.accessToken}")
        }
        try {
            if (connection.responseCode == HttpURLConnection.HTTP_UNAUTHORIZED && config.account != null) {
                connection.disconnect()
                val refreshed = refreshAccount(config.account) ?: return
                pollAlertsWithToken(config.url, refreshed, cursorKey, initializedKey, since, initialized)
                return
            }
            if (connection.responseCode != HttpURLConnection.HTTP_OK) return
            val body = connection.inputStream.bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
            if (relayConfigStillCurrent(config)) {
                applyAlerts(body, prefs, cursorKey, initializedKey, since, initialized)
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun pollAlertsWithToken(
        relayUrl: String,
        config: AccountSession,
        cursorKey: String,
        initializedKey: String,
        since: Long,
        initialized: Boolean,
    ) {
        val endpoint = "${relayUrl.trimEnd('/')}/v1/alerts?since=$since&limit=60"
        val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"; connectTimeout = 10_000; readTimeout = 10_000
            setRequestProperty("Accept", "application/json")
            setRequestProperty("Authorization", "Bearer ${config.accessToken}")
        }
        try {
            if (connection.responseCode != HttpURLConnection.HTTP_OK) return
            val body = connection.inputStream.bufferedReader(StandardCharsets.UTF_8).use { it.readText() }
            if (accountStillCurrent(config)) {
                applyAlerts(body, getSharedPreferences(PREFS, Context.MODE_PRIVATE), cursorKey, initializedKey, since, initialized)
            }
        } finally { connection.disconnect() }
    }

    private fun applyAlerts(body: String, prefs: android.content.SharedPreferences, cursorKey: String, initializedKey: String, since: Long, initialized: Boolean) {
        val alerts = JSONObject(body).optJSONArray("alerts") ?: return
        var newestSeq = since
        for (index in 0 until alerts.length()) {
            val alert = alerts.optJSONObject(index) ?: continue
            newestSeq = maxOf(newestSeq, alert.optLong("seq", newestSeq))
            val receivedAt = runCatching { Instant.parse(alert.getString("received_at")) }.getOrNull()
            val fresh = receivedAt != null && Instant.now().minusSeconds(MAX_NOTIFICATION_AGE_SECONDS).isBefore(receivedAt)
            val status = alert.optString("status")
            if (initialized && fresh && (status == "operational" || status == "test")) showRecoveryNotification(alert)
        }
        prefs.edit().putLong(cursorKey, newestSeq).putBoolean(initializedKey, true).apply()
    }

    private fun showRecoveryNotification(alert: JSONObject) {
        ensureAlertChannel()
        val status = alert.optString("status")
        val name = alert.optString("name").ifBlank { "福利渠道" }
        val latency = alert.optLong("latency_ms", -1L)
        val availability = alert.optDouble("availability_7d", Double.NaN)
        val body = buildString {
            append(if (status == "test") "Solum 通知链路正常" else "$name 已恢复")
            if (latency >= 0) append(" · ${latency}ms")
            if (availability.isFinite()) append(" · 7天 ${"%.2f".format(availability)}%")
        }
        val detailUrl = alert.optString("detail_url").takeIf { it.startsWith("https://") }
        val openIntent = detailUrl?.let { Intent(Intent.ACTION_VIEW, Uri.parse(it)) }
            ?: packageManager.getLaunchIntentForPackage(packageName)
            ?: Intent()
        val openPage = PendingIntent.getActivity(
            this,
            alert.optLong("seq", ALERT_NOTIFICATION_ID.toLong()).toInt(),
            openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(this, ALERT_CHANNEL_ID)
            .setSmallIcon(applicationInfo.icon)
            .setContentTitle(if (status == "test") "福利监控测试" else "福利渠道可以用了")
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText("$body。点击立即打开福利页面。"))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setAutoCancel(true)
            .setContentIntent(openPage)
            .build()
        try {
            val notificationId = ALERT_NOTIFICATION_ID +
                (alert.optLong("seq", 0L) % 10_000L).toInt()
            NotificationManagerCompat.from(this).notify(notificationId, notification)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS denied. The web dashboard still shows the event.
        }
    }

    private fun ensureAlertChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            ALERT_CHANNEL_ID,
            "Solum 福利提醒",
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "监控渠道恢复时立即提醒"
            enableVibration(true)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    private data class AccountSession(val serverUrl: String, val userId: String, val username: String, val accessToken: String, val refreshToken: String)
    private data class AccountRecord(val session: AccountSession, val json: JSONObject)
    private data class RotatedTokens(val accessToken: String, val refreshToken: String, val userId: String)
    private data class RefreshCommit(val session: AccountSession?, val revokeRotatedToken: Boolean)
    private data class RelayConfig(val url: String, val accessToken: String, val account: AccountSession?, val cursorScope: String)

    private fun loadSyncConfig(): RelayConfig? {
        // The account session is device-global; business configuration lives
        // under the same UUID profile as SQLite. Never fall back to the guest
        // sync file while an authenticated profile is active.
        val account = loadAccountSession()
        val file = if (account == null) File(dataDir, "solum-sync.json")
            else File(File(File(dataDir, "profiles"), account.userId), "solum-sync.json")
        if (!file.isFile) return null
        val json = runCatching { JSONObject(file.readText(StandardCharsets.UTF_8)) }.getOrNull() ?: return null
        val url = json.optString("url").trim().trimEnd('/')
        if (!isAllowedEndpoint(url)) return null
        val directToken = json.opt("token") as? String ?: ""
        if (account != null) {
            // Account access tokens are valid for the unified center only.
            // Never let an old or edited sync file redirect them to another
            // otherwise-valid HTTPS endpoint.
            if (url != account.serverUrl) return null
            return RelayConfig(url, account.accessToken, account, account.userId)
        }
        if (directToken.isNotEmpty()) return RelayConfig(url, directToken, null, "legacy")
        return null
    }

    private fun loadAccountSession(): AccountSession? {
        return runCatching {
            withAccountLock { loadAccountRecordUnlocked()?.session }
        }.getOrNull()
    }

    private fun loadAccountRecordUnlocked(): AccountRecord? {
        val file = File(dataDir, "solum-account.json")
        if (!file.isFile) return null
        val json = runCatching { JSONObject(file.readText(StandardCharsets.UTF_8)) }.getOrNull() ?: return null
        val serverUrl = (json.opt("server_url") as? String)?.trim()?.trimEnd('/') ?: return null
        val userId = (json.opt("user_id") as? String)?.trim()?.lowercase() ?: return null
        val username = (json.opt("username") as? String)?.trim() ?: return null
        val access = json.opt("access_token") as? String ?: return null
        val refresh = json.opt("refresh_token") as? String ?: return null
        if (!isAllowedEndpoint(serverUrl) || !isStableUserId(userId) || username.isEmpty() || access.isEmpty() || refresh.isEmpty()) return null
        return AccountRecord(AccountSession(serverUrl, userId, username, access, refresh), json)
    }

    private fun refreshAccount(session: AccountSession): AccountSession? {
        // Snapshot the newest same-identity session while synchronized with
        // Rust. The potentially slow request deliberately happens outside the
        // lock so logout and account switching remain responsive.
        val base = runCatching {
            withAccountLock {
                loadAccountRecordUnlocked()?.session?.takeIf { sameIdentity(it, session) }
            }
        }.getOrNull() ?: return null
        if (base.refreshToken != session.refreshToken) return base

        val rotated = requestRefresh(base)
        if (rotated == null) {
            // Another process may have consumed the single-use token and
            // already published its replacement while this request was live.
            return runCatching {
                withAccountLock {
                    loadAccountRecordUnlocked()?.session?.takeIf {
                        sameIdentity(it, base) && it.refreshToken != base.refreshToken
                    }
                }
            }.getOrNull()
        }

        if (rotated.userId != base.userId) {
            revokeRefreshToken(base.serverUrl, rotated.refreshToken)
            return null
        }

        val committed = runCatching {
            withAccountLock {
                val current = loadAccountRecordUnlocked()
                if (current == null || !sameIdentity(current.session, base)) {
                    RefreshCommit(null, true)
                } else if (current.session.refreshToken != base.refreshToken) {
                    RefreshCommit(current.session, true)
                } else {
                    // Keep model and any future/unknown fields from the latest
                    // local record. Only the server-owned token pair rotates.
                    current.json.put("access_token", rotated.accessToken)
                    current.json.put("refresh_token", rotated.refreshToken)
                    writeAccountRecordUnlocked(current.json)
                    RefreshCommit(
                        current.session.copy(
                            accessToken = rotated.accessToken,
                            refreshToken = rotated.refreshToken,
                        ),
                        false,
                    )
                }
            }
        }.getOrElse { RefreshCommit(null, true) }

        if (committed.revokeRotatedToken) {
            revokeRefreshToken(base.serverUrl, rotated.refreshToken)
        }
        return committed.session
    }

    private fun requestRefresh(session: AccountSession): RotatedTokens? {
        repeat(2) {
            val attempt = runCatching { requestRefreshOnce(session) }
            // HTTP rejections and structurally invalid successful responses
            // return null without retrying. Only transport/JSON exceptions get
            // one retry, which the center's idempotent rotation can recover.
            if (attempt.isSuccess) return attempt.getOrNull()
        }
        return null
    }

    private fun requestRefreshOnce(session: AccountSession): RotatedTokens? {
        val connection = (URL("${session.serverUrl}/v1/auth/refresh").openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"; doOutput = true; connectTimeout = 10_000; readTimeout = 10_000
            setRequestProperty("Content-Type", "application/json")
        }
        return try {
            connection.outputStream.use { it.write(JSONObject().put("refresh_token", session.refreshToken).toString().toByteArray(StandardCharsets.UTF_8)) }
            if (connection.responseCode != HttpURLConnection.HTTP_OK) return null
            val json = connection.inputStream.bufferedReader(StandardCharsets.UTF_8).use { JSONObject(it.readText()) }
            val accessToken = json.opt("access_token") as? String ?: return null
            val refreshToken = json.opt("refresh_token") as? String ?: return null
            val userId = (json.optJSONObject("user")?.opt("id") as? String)
                ?.trim()?.lowercase() ?: return null
            if (accessToken.isEmpty() || refreshToken.isEmpty() || !isStableUserId(userId)) return null
            RotatedTokens(accessToken, refreshToken, userId)
        } finally { connection.disconnect() }
    }

    private fun revokeRefreshToken(serverUrl: String, refreshToken: String) {
        val connection = runCatching {
            (URL("$serverUrl/v1/auth/logout").openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"; doOutput = true; connectTimeout = 5_000; readTimeout = 5_000
                setRequestProperty("Content-Type", "application/json")
            }
        }.getOrNull() ?: return
        try {
            runCatching {
                connection.outputStream.use {
                    it.write(JSONObject().put("refresh_token", refreshToken).toString().toByteArray(StandardCharsets.UTF_8))
                }
                connection.responseCode
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun sameIdentity(left: AccountSession, right: AccountSession): Boolean =
        left.serverUrl == right.serverUrl &&
            left.userId == right.userId

    private fun accountStillCurrent(expected: AccountSession): Boolean = runCatching {
        withAccountLock { loadAccountRecordUnlocked()?.session?.let { sameIdentity(it, expected) } == true }
    }.getOrDefault(false)

    private fun relayConfigStillCurrent(expected: RelayConfig): Boolean {
        val current = loadSyncConfig() ?: return false
        if (current.url != expected.url || current.cursorScope != expected.cursorScope) return false
        return if (expected.account != null && current.account != null) {
            sameIdentity(current.account, expected.account)
        } else {
            expected.account == null && current.account == null &&
                current.accessToken == expected.accessToken
        }
    }

    private fun <T> withAccountLock(operation: () -> T): T {
        return synchronized(ACCOUNT_LOCK_MONITOR) {
            val lockFile = File(dataDir, "solum-account.json.lock")
            lockFile.parentFile?.mkdirs()
            RandomAccessFile(lockFile, "rw").use { handle ->
                val lock = handle.channel.lock()
                try {
                    operation()
                } finally {
                    lock.release()
                }
            }
        }
    }

    private fun writeAccountRecordUnlocked(json: JSONObject) {
        val target = File(dataDir, "solum-account.json")
        val atomicFile = AtomicFile(target)
        val payload = json.toString(2).toByteArray(StandardCharsets.UTF_8)
        var output: FileOutputStream? = null
        try {
            output = atomicFile.startWrite()
            output.write(payload)
            atomicFile.finishWrite(output)
            output = null
            if (!target.isFile || !target.readBytes().contentEquals(payload)) {
                throw IOException("account session commit could not be verified")
            }
        } catch (error: Exception) {
            output?.let { runCatching { atomicFile.failWrite(it) } }
            throw error
        }
    }

    private fun isAllowedEndpoint(value: String): Boolean = runCatching {
        val uri = URI(value)
        if (!uri.isAbsolute || uri.isOpaque || uri.rawUserInfo != null ||
            uri.rawQuery != null || uri.rawFragment != null) return@runCatching false
        val host = uri.host ?: return@runCatching false
        uri.scheme.equals("https", ignoreCase = true) ||
            (uri.scheme.equals("http", ignoreCase = true) && isLoopbackHost(host))
    }.getOrDefault(false)

    private fun isLoopbackHost(host: String): Boolean {
        if (host.equals("localhost", ignoreCase = true) || host == "::1" || host == "[::1]") {
            return true
        }
        val parts = host.split('.')
        if (parts.size != 4 || parts[0] != "127") return false
        return parts.all { part -> part.isNotEmpty() && part.all(Char::isDigit) &&
            part.toIntOrNull()?.let { it in 0..255 } == true }
    }

    private fun isStableUserId(value: String): Boolean =
        Regex("^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$").matches(value)


    companion object {
        private const val CHANNEL_ID = "solum_notification_pipeline"
        /// Pre-rename channel id, deleted on every channel ensure so upgraded
        /// installs do not keep a dead entry in system settings.
        private const val LEGACY_CHANNEL_ID = "pa_notification_pipeline"
        private const val NOTIFICATION_ID = 41020
        private const val ALERT_CHANNEL_ID = "solum_benefit_alerts"
        private const val ALERT_NOTIFICATION_ID = 41021
        private const val PREFS = "solum_alert_relay"
        private const val KEY_CURSOR = "cursor"
        private const val KEY_INITIALIZED = "initialized"
        private const val POLL_SECONDS = 20L
        private const val MAX_NOTIFICATION_AGE_SECONDS = 10 * 60L
        private val ACCOUNT_LOCK_MONITOR = Any()

        @Volatile var running: Boolean = false
            private set

        fun start(context: Context) {
            ContextCompat.startForegroundService(context, Intent(context, NotificationPipelineService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, NotificationPipelineService::class.java))
        }
    }
}
