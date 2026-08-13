package dev.solum.app

import android.app.Notification
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import org.json.JSONObject
import java.io.File
import java.util.ArrayDeque

/**
 * F1: capture device notifications and hand them to the Rust core.
 *
 * F20 keeps this native edge deliberately dumb, but it does enforce the
 * default-empty app whitelist *before* anything reaches the inbox. The tiny
 * `notif-policy.json` projection is written by Rust beside the database.
 * All routing, deduplication, cloud gating and event creation remain in Rust.
 *
 * Requires the user to grant notification access (Settings → Notifications →
 * Notification access; on the emulator:
 * `adb shell cmd notification allow_listener dev.solum.app/dev.solum.app.SolumNotificationListener`).
 */
class SolumNotificationListener : NotificationListenerService() {
    /**
     * Short-lived identities of notifications already appended to the inbox.
     * `StatusBarNotification.key` identifies one posted notification; pairing
     * it with content suppresses its noisy duplicate callbacks without
     * suppressing a later, identical daily notification from the same app.
     */
    private val recent = ArrayDeque<Pair<String, Long>>()
    private companion object {
        const val RECENT_TTL_MS = 5 * 60 * 1000L
        const val RECENT_MAX = 64

        /**
         * Per-field ceiling, mirroring `notification_intelligence::MAX_FIELD_CHARS`.
         * Notification text comes from another app and can be arbitrarily long;
         * truncating here means the oversized text never touches our disk at
         * all, rather than being written and trimmed later.
         */
        const val MAX_FIELD_CHARS = 2_000

        /** Spool directory: one file per captured notification. */
        const val SPOOL_DIR = "notif-spool"

        /**
         * Stop spooling past this many pending files. The drain runs on a
         * one-minute tick, so a healthy device never approaches it; reaching it
         * means nothing is draining (force-stopped, storage full) and the right
         * response is to stop consuming the user's disk rather than grow without
         * limit. Capture is best-effort by design — silence here is recoverable,
         * a full disk is not.
         */
        const val MAX_SPOOL_FILES = 5_000

        /**
         * One immutable marker file per dropped notification. A directory of
         * markers rather than a shared counter file — see [noteOverflow].
         */
        const val OVERFLOW_DIR = "notif-overflow"
        const val MARK_EXT = ".mark"

        /** Markers are bounded too; past this the loss is reported as "at least N". */
        const val MAX_OVERFLOW_MARKERS = 2_000

        /** A `.tmp` older than this is abandoned work from a crashed write. */
        const val STALE_TMP_MS = 5 * 60 * 1000L
    }

    /**
     * Record one dropped notification as **its own immutable marker file**:
     * write `<stem>.tmp`, fsync, rename to `<stem>.mark`.
     *
     * No shared mutable counter file, because every version of that idea has a
     * cross-process race and the fixes only move it:
     *  - *read total, write total+1* races the drainer's read outright;
     *  - *append one byte* removes the stale-read problem but not this one:
     *    this process can open the live file, the drainer can then rename it
     *    aside, read its length, record it and unlink it, and only then does
     *    our `write(1)` land — on an inode nobody will ever read again. The
     *    byte is in neither the claimed file nor the new live file. An
     *    `fsync` does not help; the file descriptor was resolved before the
     *    rename and the write happened after the read.
     *
     * A marker has no shared mutable state to race over. It is created by an
     * atomic rename, so the drainer either sees a whole marker or nothing, and
     * it deletes only the exact files it counted — a marker that appears
     * mid-drain is simply counted on the next pass. Same shape as the spool
     * itself, which is what this should have been from the start.
     */
    private fun noteOverflow() {
        synchronized(spoolLock) {
            try {
                val dir = File(dataDir, OVERFLOW_DIR)
                if (!dir.isDirectory) dir.mkdirs()

                val sweepAt = System.currentTimeMillis()
                dir.listFiles()?.forEach { f ->
                    if (f.name.endsWith(".tmp") && sweepAt - f.lastModified() > STALE_TMP_MS) {
                        f.delete()
                    }
                }
                // Bounded like everything else: if nothing is draining, markers
                // must not grow without limit either. Past the cap the loss is
                // undercounted, and the drainer says "at least N" rather than
                // claiming a precise figure it cannot have.
                val existing = dir.list { _, name -> name.endsWith(MARK_EXT) }?.size ?: 0
                if (existing >= MAX_OVERFLOW_MARKERS) return

                val stem = "$sweepAt-${java.util.UUID.randomUUID().toString().take(8)}"
                val tmp = File(dir, "$stem.tmp")
                java.io.FileOutputStream(tmp).use { out ->
                    out.write(1)
                    out.fd.sync()
                }
                if (!tmp.renameTo(File(dir, "$stem$MARK_EXT"))) tmp.delete()
            } catch (_: Exception) {
                // Best-effort: never crash the listener over a counter.
            }
        }
    }

    /**
     * Guards the whole spool admission decision — sweep, count, write, rename —
     * not just the counter.
     *
     * `onNotificationPosted` can be delivered concurrently, so checking the
     * quota outside a lock let several callers all observe 4,999 and then all
     * write, putting the directory over its cap. The check and the act that
     * depends on it have to be one critical section, or the "limit" is only a
     * suggestion.
     */
    private val spoolLock = Any()

    private fun clamp(value: String): String =
        if (value.length <= MAX_FIELD_CHARS) value
        else value.substring(0, MAX_FIELD_CHARS) + "…（已截断）"

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        // Never capture our own reminders — that would loop them back as input.
        if (sbn.packageName == packageName) return
        // Ongoing/foreground-service noise (music players, downloads) is not "a message".
        if (sbn.isOngoing) return
        // Default empty/invalid policy means no capture. This is intentionally
        // checked before reading notification extras or building an inbox row.
        if (!isAllowed(sbn.packageName)) return

        val extras = sbn.notification.extras
        val title = clamp(extras.getCharSequence(Notification.EXTRA_TITLE)?.toString().orEmpty())
        val text = clamp(extras.getCharSequence(Notification.EXTRA_TEXT)?.toString().orEmpty())
        if (title.isBlank() && text.isBlank()) return

        val identity = sbn.key + "\u0000" + title + "\u0000" + text
        val now = System.currentTimeMillis()
        synchronized(recent) {
            while (recent.firstOrNull()?.second?.let { now - it > RECENT_TTL_MS } == true) {
                recent.removeFirst()
            }
            if (recent.any { it.first == identity }) return
            recent.addLast(identity to now)
            while (recent.size > RECENT_MAX) recent.removeFirst()
        }

        val line = JSONObject()
            .put("ts", sbn.postTime)
            .put("pkg", sbn.packageName)
            .put("title", title)
            .put("text", text)
            .toString()
        try {
            // **One notification, one file.** Appending to a shared JSONL cannot
            // be made safe across two processes: the drainer renames the file
            // aside, but an append already in flight keeps writing to the old
            // inode (that data is then deleted along with the claim file), and a
            // partially-flushed append leaves a half line that parses as
            // garbage. Both lose notifications, silently.
            //
            // A spool directory removes the coordination problem instead of
            // trying to win it: write to `.tmp`, fsync, rename to `.json`.
            // Rename is atomic, so the drainer only ever sees whole files, and
            // one it has not picked up yet is simply read on the next tick.
            //
            // Separately: the quota decision and the write it authorizes are
            // ONE critical section. Checking outside the lock let several
            // concurrent deliveries all observe 4,999 and all proceed — the cap
            // would be exceeded by however many happened to race.
            var overflowed = false
            synchronized(spoolLock) {
                val spool = File(dataDir, SPOOL_DIR)
                if (!spool.isDirectory) spool.mkdirs()

                // Sweep abandoned `.tmp` files before measuring. A crash
                // between create and rename leaves one behind, and counting
                // those toward the quota would let a handful of stale
                // temporaries permanently consume the budget — capture would
                // then stop for a reason nothing on screen could explain.
                // (Distinct from `now` above, which stamps the dedup window.)
                val sweepAt = System.currentTimeMillis()
                spool.listFiles()?.forEach { f ->
                    if (f.name.endsWith(".tmp") && sweepAt - f.lastModified() > STALE_TMP_MS) {
                        f.delete()
                    }
                }

                // Only complete spool entries count against the cap.
                val pending = spool.list { _, name -> name.endsWith(".json") }?.size ?: 0
                if (pending >= MAX_SPOOL_FILES) {
                    overflowed = true
                } else {
                    val stem = "${sbn.postTime}-${java.util.UUID.randomUUID().toString().take(8)}"
                    val tmp = File(spool, "$stem.tmp")
                    java.io.FileOutputStream(tmp).use { out ->
                        out.write((line + "\n").toByteArray())
                        out.fd.sync()
                    }
                    if (!tmp.renameTo(File(spool, "$stem.json"))) {
                        tmp.delete()
                    }
                }
            }
            // Outside the lock only because `noteOverflow` takes it itself;
            // dropping is survivable, dropping *silently* is not.
            if (overflowed) noteOverflow()
        } catch (_: Exception) {
            // Capture is best-effort; never crash the listener.
        }
    }

    private fun isAllowed(packageName: String): Boolean {
        return try {
            val policy = activeProfileFile("notif-policy.json")
            if (!policy.isFile) false
            else {
                val packages = JSONObject(policy.readText()).optJSONArray("allowed_packages")
                packages != null && (0 until packages.length()).any { packages.optString(it) == packageName }
            }
        } catch (_: Exception) {
            false
        }
    }

    private fun activeProfileFile(name: String): File {
        val account = File(dataDir, "solum-account.json")
        val userId = runCatching {
            JSONObject(account.readText()).optString("user_id").trim().lowercase()
        }.getOrDefault("")
        return if (Regex("^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$").matches(userId)) {
            File(File(File(dataDir, "profiles"), userId), name)
        } else {
            File(dataDir, name)
        }
    }
}
