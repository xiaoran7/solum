package dev.solum.alarm

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * Fires at a reminder's `fire_at`, possibly with the 息壤 process long dead:
 * posts the OS notification the ticker would have posted. Deliberately dumb
 * (F16): no DB access, no Rust — the strings were baked into the intent at
 * scheduling time. Reminder *state* (mark-fired + journal) is still owned
 * exclusively by `fire_due` on the next app run; on Android the ticker
 * skips its own OS toast so this receiver is the single delivery surface.
 */
class AlarmReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != AlarmScheduler.ACTION_FIRE) return
        val id = intent.getLongExtra("id", -1L)
        val title = intent.getStringExtra("title") ?: "息壤 提醒"
        val body = intent.getStringExtra("body") ?: ""

        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "息壤 提醒", NotificationManager.IMPORTANCE_HIGH)
        )

        val launch = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName)
        val contentPi = launch?.let {
            PendingIntent.getActivity(
                ctx, 0, it,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }
        val notif = NotificationCompat.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(ctx.applicationInfo.icon)
            .setContentTitle(title)
            .setContentText(body)
            .setAutoCancel(true)
            .apply { contentPi?.let { setContentIntent(it) } }
            .build()
        try {
            NotificationManagerCompat.from(ctx).notify((id and 0x7fffffff).toInt(), notif)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS denied: nothing to do, in-app surfaces still
            // show the reminder on next launch.
        }

        // One-shot: drop it from the persisted set so reboot won't re-arm it.
        if (id >= 0) AlarmScheduler.remove(ctx, id)
    }

    companion object {
        private const val CHANNEL_ID = "solum_reminders"
    }
}
