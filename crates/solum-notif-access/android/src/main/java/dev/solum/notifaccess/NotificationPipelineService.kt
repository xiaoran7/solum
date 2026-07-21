package dev.solum.notifaccess

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat

/**
 * F20's foreground-service skeleton. The service deliberately does not parse
 * notifications or call the network: it keeps the *same application process*
 * that hosts Tauri/Rust alive, where the resident ticker owns the configured
 * 15–30 minute batch cadence and all privacy/LLM decisions.
 */
class NotificationPipelineService : Service() {
    override fun onCreate() {
        super.onCreate()
        running = true
        ensureChannel()
        startForeground(NOTIFICATION_ID, buildNotification())
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
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

    companion object {
        private const val CHANNEL_ID = "pa_notification_pipeline"
        private const val NOTIFICATION_ID = 41020

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
