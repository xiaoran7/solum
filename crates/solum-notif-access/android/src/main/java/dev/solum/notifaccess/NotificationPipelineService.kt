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
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.nio.charset.StandardCharsets
import java.time.Instant
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * F20's foreground-service skeleton. The service deliberately does not parse
 * notifications or call the network: it keeps the *same application process*
 * that hosts Tauri/Rust alive, where the resident ticker owns the configured
 * 15–30 minute batch cadence and all privacy/LLM decisions.
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
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Solum 通知处理",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "保持 Solum 在后台处理你已授权应用的通知"
            setShowBadge(false)
        }
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
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
            applyAlerts(body, prefs, cursorKey, initializedKey, since, initialized)
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
            applyAlerts(body, getSharedPreferences(PREFS, Context.MODE_PRIVATE), cursorKey, initializedKey, since, initialized)
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
        val directToken = json.optString("token").trim()
        if (account != null) return RelayConfig(url, account.accessToken, account, account.userId)
        if (directToken.isNotEmpty()) return RelayConfig(url, directToken, null, "legacy")
        return null
    }

    private fun loadAccountSession(): AccountSession? {
        val file = File(dataDir, "solum-account.json")
        if (!file.isFile) return null
        val json = runCatching { JSONObject(file.readText(StandardCharsets.UTF_8)) }.getOrNull() ?: return null
        val serverUrl = json.optString("server_url").trim().trimEnd('/')
        val userId = json.optString("user_id").trim().lowercase()
        val username = json.optString("username").trim()
        val access = json.optString("access_token").trim()
        val refresh = json.optString("refresh_token").trim()
        if (!isAllowedEndpoint(serverUrl) || !isStableUserId(userId) || username.isEmpty() || access.isEmpty() || refresh.isEmpty()) return null
        return AccountSession(serverUrl, userId, username, access, refresh)
    }

    private fun refreshAccount(session: AccountSession): AccountSession? {
        val connection = (URL("${session.serverUrl}/v1/auth/refresh").openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"; doOutput = true; connectTimeout = 10_000; readTimeout = 10_000
            setRequestProperty("Content-Type", "application/json")
        }
        return try {
            connection.outputStream.use { it.write(JSONObject().put("refresh_token", session.refreshToken).toString().toByteArray(StandardCharsets.UTF_8)) }
            if (connection.responseCode != HttpURLConnection.HTTP_OK) {
                // Rust/AI may have won a concurrent single-use refresh-token
                // rotation. Re-read its atomically persisted replacement.
                return loadAccountSession()?.takeIf {
                    it.userId == session.userId && it.refreshToken != session.refreshToken
                }
            }
            val json = connection.inputStream.bufferedReader(StandardCharsets.UTF_8).use { JSONObject(it.readText()) }
            val refreshedUserId = json.optJSONObject("user")?.optString("id")?.trim()?.lowercase().orEmpty()
            if (refreshedUserId != session.userId) return null
            val rotated = session.copy(accessToken = json.optString("access_token"), refreshToken = json.optString("refresh_token"))
            if (rotated.accessToken.isEmpty() || rotated.refreshToken.isEmpty()) return null
            val file = File(dataDir, "solum-account.json")
            val tmp = File(file.parentFile, "${file.name}.tmp")
            val persisted = runCatching { JSONObject(file.readText(StandardCharsets.UTF_8)) }.getOrElse { JSONObject() }
            persisted.put("server_url", rotated.serverUrl).put("username", rotated.username)
                .put("access_token", rotated.accessToken).put("refresh_token", rotated.refreshToken)
            tmp.writeText(persisted.toString(2), StandardCharsets.UTF_8)
            if (!tmp.renameTo(file)) { tmp.delete(); return null }
            rotated
        } finally { connection.disconnect() }
    }

    private fun isAllowedEndpoint(value: String): Boolean = runCatching {
        val uri = URI(value)
        uri.scheme.equals("https", ignoreCase = true) ||
            (uri.scheme.equals("http", ignoreCase = true) &&
                (uri.host == "127.0.0.1" || uri.host == "localhost" || uri.host == "::1"))
    }.getOrDefault(false)

    private fun isStableUserId(value: String): Boolean =
        Regex("^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$").matches(value)


    companion object {
        private const val CHANNEL_ID = "pa_notification_pipeline"
        private const val NOTIFICATION_ID = 41020
        private const val ALERT_CHANNEL_ID = "solum_benefit_alerts"
        private const val ALERT_NOTIFICATION_ID = 41021
        private const val PREFS = "solum_alert_relay"
        private const val KEY_CURSOR = "cursor"
        private const val KEY_INITIALIZED = "initialized"
        private const val POLL_SECONDS = 20L
        private const val MAX_NOTIFICATION_AGE_SECONDS = 10 * 60L

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
