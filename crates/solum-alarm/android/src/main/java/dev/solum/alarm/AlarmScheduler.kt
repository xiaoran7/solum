package dev.solum.alarm

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/**
 * The single owner of Solum's OS alarm set. The Rust ticker pushes the whole
 * pending-reminder set on every change ([replaceAll]); reboot re-arms from
 * the persisted schedule file ([applyAll] via [BootReceiver]). All state
 * lives in one JSON file next to solum.sqlite (`dataDir`, same location the
 * notification-capture inbox uses) — the OS alarm registry is treated as a
 * disposable mirror of it.
 */
object AlarmScheduler {
    private const val FILE = "alarm-schedule.json"
    const val ACTION_FIRE = "dev.solum.alarm.FIRE"

    data class Item(val id: Long, val atMs: Long, val title: String, val body: String)

    private fun scheduleFile(ctx: Context) = File(ctx.dataDir, FILE)

    @Synchronized
    fun load(ctx: Context): List<Item> {
        val f = scheduleFile(ctx)
        if (!f.exists()) return emptyList()
        return try {
            val arr = JSONArray(f.readText())
            (0 until arr.length()).map { i ->
                val o = arr.getJSONObject(i)
                Item(o.getLong("id"), o.getLong("atMs"), o.getString("title"), o.getString("body"))
            }
        } catch (_: Exception) {
            emptyList()
        }
    }

    /**
     * Persist the schedule atomically, and **report failure**.
     *
     * This used to swallow the exception. That turned a disk error into a
     * silent lie: [replaceAll] had already cancelled the old alarms, so a
     * failed save left the file holding a schedule that no longer matches
     * reality, while the caller was told the sync succeeded and recorded a
     * new `alarm_sig` — meaning it would never retry. After a reboot the
     * device then re-arms the stale set: new reminders missing, cancelled
     * ones resurrected.
     *
     * Write-temp-then-rename also matters here: `writeText` truncates first,
     * so a crash mid-write leaves a half-written JSON array that [load]
     * cannot parse and silently treats as *no alarms at all*.
     */
    @Synchronized
    private fun save(ctx: Context, items: List<Item>): Boolean {
        val arr = JSONArray()
        for (i in items) {
            arr.put(JSONObject().put("id", i.id).put("atMs", i.atMs).put("title", i.title).put("body", i.body))
        }
        val target = scheduleFile(ctx)
        val tmp = File(ctx.dataDir, "$FILE.tmp")
        return try {
            tmp.outputStream().use { out ->
                out.write(arr.toString().toByteArray())
                out.fd.sync()
            }
            if (!tmp.renameTo(target)) {
                tmp.delete()
                return false
            }
            true
        } catch (_: Exception) {
            try { tmp.delete() } catch (_: Exception) {}
            false
        }
    }

    fun canExact(ctx: Context): Boolean {
        val am = ctx.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        return Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms()
    }

    /**
     * Cancel everything previously armed, persist and arm the new set.
     *
     * Throws if the schedule could not be persisted. The caller must not
     * record this set as synced in that case — see [save].
     */
    fun replaceAll(ctx: Context, items: List<Item>): Boolean {
        val am = ctx.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        // Read the outgoing set *before* the save overwrites the file — that
        // is the only record of which PendingIntents need cancelling.
        val previous = load(ctx)
        // Persist before cancelling: if the write fails we have not yet
        // dismantled the working alarm set, so the device keeps the reminders
        // it already had rather than ending up with neither set.
        if (!save(ctx, items)) {
            throw java.io.IOException("无法保存提醒计划（$FILE），未改动已挂载的闹钟")
        }
        for (old in previous) am.cancel(pending(ctx, old))
        return applyAll(ctx, items)
    }

    /** Arm all future items (used by both sync and boot re-arm). */
    fun applyAll(ctx: Context, items: List<Item> = load(ctx)): Boolean {
        val am = ctx.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        val now = System.currentTimeMillis()
        val exact = canExact(ctx)
        for (i in items) {
            if (i.atMs <= now) continue // overdue: fire_due picks it up on next launch
            val pi = pending(ctx, i)
            try {
                if (exact) am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, i.atMs, pi)
                else am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, i.atMs, pi)
            } catch (_: SecurityException) {
                // Exact access revoked mid-flight: degrade, never crash.
                am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, i.atMs, pi)
            }
        }
        return exact
    }

    /** Drop one fired item from the persisted set (so reboot won't re-arm it). */
    @Synchronized
    fun remove(ctx: Context, id: Long) {
        save(ctx, load(ctx).filter { it.id != id })
    }

    /**
     * The request code is the solum-core notification row id, so the same
     * reminder always maps to the same PendingIntent (replace, not
     * duplicate) and cancel-by-id works without extras matching.
     */
    private fun pending(ctx: Context, item: Item): PendingIntent {
        val intent = Intent(ctx, AlarmReceiver::class.java)
            .setAction(ACTION_FIRE)
            .putExtra("id", item.id)
            .putExtra("title", item.title)
            .putExtra("body", item.body)
        return PendingIntent.getBroadcast(
            ctx,
            (item.id and 0x7fffffff).toInt(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
