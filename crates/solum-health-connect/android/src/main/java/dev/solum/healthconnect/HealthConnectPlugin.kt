package dev.solum.healthconnect

import android.app.Activity
import androidx.activity.result.ActivityResult
import androidx.health.connect.client.HealthConnectClient
import androidx.health.connect.client.PermissionController
import androidx.health.connect.client.permission.HealthPermission
import androidx.health.connect.client.records.HeartRateRecord
import androidx.health.connect.client.records.Record
import androidx.health.connect.client.records.SleepSessionRecord
import androidx.health.connect.client.records.StepsRecord
import androidx.health.connect.client.request.AggregateRequest
import androidx.health.connect.client.request.ReadRecordsRequest
import androidx.health.connect.client.time.TimeRangeFilter
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import java.time.Duration
import java.time.Instant
import kotlin.reflect.KClass

@InvokeArg
class ReadRecentArgs {
    var sinceEpochMs: Long = 0
}

/**
 * F5 v1 (Phase 4, ARCHITECTURE.md §3.7): read-only Health Connect adapter.
 * Samsung Health (and any other vendor app) writes into Health Connect; we
 * never talk to a vendor SDK directly — only three record types, only READ
 * permissions, no write-back.
 */
@TauriPlugin
class HealthConnectPlugin(private val activity: Activity) : Plugin(activity) {
    private val scope = CoroutineScope(Dispatchers.IO)

    private val permissions = setOf(
        HealthPermission.getReadPermission(HeartRateRecord::class),
        HealthPermission.getReadPermission(StepsRecord::class),
        HealthPermission.getReadPermission(SleepSessionRecord::class),
    )
    private val permissionContract = PermissionController.createRequestPermissionResultContract()

    private companion object {
        /** Records kept per record type in one read. */
        const val MAX_RECORDS = 20_000
        /** Pages fetched per record type, as a second brake. */
        const val MAX_PAGES = 50
        /** Longest history one `readRecent` may ask for. */
        const val MAX_LOOKBACK_DAYS = 7L
        /** Heart-rate sub-samples emitted across the bridge in one response. */
        const val MAX_EMITTED_SAMPLES = 50_000
    }

    private fun client(): HealthConnectClient? =
        if (HealthConnectClient.getSdkStatus(activity) == HealthConnectClient.SDK_AVAILABLE)
            HealthConnectClient.getOrCreate(activity)
        else null

    /**
     * Read pages up to a bounded number of records.
     *
     * The unbounded version accumulated *every* page in memory, and the
     * command accepts an arbitrary start instant — so a caller asking for a
     * year of a high-frequency heart-rate feed would materialize the lot,
     * serialize it across the IPC bridge, and hand it to the JS side as one
     * array. The normal poll window is six hours; these caps are far above
     * that and exist so an abnormal request degrades into "some data" rather
     * than into an OOM.
     */
    private suspend fun <T : Record> readAll(
        client: HealthConnectClient,
        type: KClass<T>,
        range: TimeRangeFilter,
    ): List<T> {
        val out = mutableListOf<T>()
        var pageToken: String? = null
        var pages = 0
        do {
            val response = client.readRecords(
                ReadRecordsRequest(
                    recordType = type,
                    timeRangeFilter = range,
                    pageToken = pageToken,
                )
            )
            out += response.records
            pageToken = response.pageToken
            pages += 1
            if (out.size >= MAX_RECORDS || pages >= MAX_PAGES) break
        } while (pageToken != null)
        return if (out.size > MAX_RECORDS) out.subList(0, MAX_RECORDS) else out
    }

    @Command
    fun isAvailable(invoke: Invoke) {
        val ret = JSObject()
        ret.put("available", client() != null)
        invoke.resolve(ret)
    }

    @Command
    fun hasPermissions(invoke: Invoke) {
        val c = client()
        if (c == null) {
            invoke.resolve(JSObject().apply { put("granted", false) })
            return
        }
        scope.launch {
            val granted = c.permissionController.getGrantedPermissions()
            invoke.resolve(JSObject().apply { put("granted", granted.containsAll(permissions)) })
        }
    }

    @Command
    override fun requestPermissions(invoke: Invoke) {
        if (client() == null) {
            invoke.reject("Health Connect 不可用")
            return
        }
        val intent = permissionContract.createIntent(activity, permissions)
        startActivityForResult(invoke, intent, "handlePermissionResult")
    }

    @ActivityCallback
    fun handlePermissionResult(invoke: Invoke, result: ActivityResult) {
        val granted = permissionContract.parseResult(result.resultCode, result.data)
        invoke.resolve(JSObject().apply { put("granted", granted.containsAll(permissions)) })
    }

    @Command
    fun readRecent(invoke: Invoke) {
        val args = invoke.parseArgs(ReadRecentArgs::class.java)
        val c = client()
        if (c == null) {
            invoke.reject("Health Connect 不可用")
            return
        }
        val end = Instant.now()
        // Clamp the look-back. `sinceEpochMs` comes in over the bridge and is
        // not otherwise constrained; a zero or malformed value would ask for
        // everything since 1970.
        val earliest = end.minusSeconds(MAX_LOOKBACK_DAYS * 24L * 3600L)
        var start = Instant.ofEpochMilli(args.sinceEpochMs)
        if (start.isBefore(earliest)) start = earliest
        if (!start.isBefore(end)) start = end.minusSeconds(60)
        val range = TimeRangeFilter.between(start, end)
        scope.launch {
            try {
                val out = JSArray()
                // One heart-rate *record* carries many sub-samples, so the
                // record cap alone does not bound what crosses the bridge.
                var emitted = 0
                outer@ for (rec in readAll(c, HeartRateRecord::class, range)) {
                    for (sample in rec.samples) {
                        if (emitted >= MAX_EMITTED_SAMPLES) break@outer
                        out.put(JSObject().apply {
                            put("kind", "heart_rate")
                            put("value", sample.beatsPerMinute.toDouble())
                            put("start", sample.time.toString())
                            put("end", sample.time.toString())
                        })
                        emitted += 1
                    }
                }
                // Steps are a cumulative metric. Aggregate across data origins
                // instead of summing raw records, which can double-count the
                // same walk imported by both a phone and a wearable.
                val stepCount = c.aggregate(
                    AggregateRequest(
                        metrics = setOf(StepsRecord.COUNT_TOTAL),
                        timeRangeFilter = range,
                    )
                )[StepsRecord.COUNT_TOTAL] ?: 0L
                out.put(JSObject().apply {
                    put("kind", "steps")
                    put("value", stepCount.toDouble())
                    put("start", start.toString())
                    put("end", end.toString())
                })
                for (rec in readAll(c, SleepSessionRecord::class, range)) {
                    out.put(JSObject().apply {
                        put("kind", "sleep")
                        put("value", Duration.between(rec.startTime, rec.endTime).toMinutes().toDouble())
                        put("start", rec.startTime.toString())
                        put("end", rec.endTime.toString())
                    })
                }
                invoke.resolve(JSObject().apply { put("samples", out) })
            } catch (e: Exception) {
                invoke.reject(e.message ?: "读取失败")
            }
        }
    }
}
