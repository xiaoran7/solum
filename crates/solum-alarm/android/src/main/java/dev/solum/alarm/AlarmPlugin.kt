package dev.solum.alarm

import android.app.Activity
import android.content.ComponentName
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class AlarmItemArg {
    var id: Long = 0
    var atMs: Long = 0
    var title: String = ""
    var body: String = ""
}

@InvokeArg
class SyncArgs {
    var alarms: List<AlarmItemArg> = emptyList()
}

/**
 * F2/F16 companion (ARCHITECTURE.md §3.1 reliability path): one command,
 * `sync`, which replaces the whole OS alarm set with the pending-reminder
 * set the Rust ticker computed. All scheduling logic lives in
 * [AlarmScheduler]; this class is only the Tauri bridge.
 */
@TauriPlugin
class AlarmPlugin(private val activity: Activity) : Plugin(activity) {
    private val startupReceiverRegistered = receiverRegistered(AlarmReceiver::class.java)
    private val startupBootReceiverRegistered = receiverRegistered(BootReceiver::class.java)

    init {
        if (!startupReceiverRegistered) {
            Log.e("SolumAlarm", "P1: AlarmReceiver is missing or disabled in the installed manifest")
        }
        if (!startupBootReceiverRegistered) {
            Log.e("SolumAlarm", "P1: BootReceiver is missing or disabled in the installed manifest")
        }
    }

    @Suppress("DEPRECATION")
    private fun receiverRegistered(receiverClass: Class<*>): Boolean = try {
        val component = ComponentName(activity, receiverClass)
        val info = if (Build.VERSION.SDK_INT >= 33) {
            activity.packageManager.getReceiverInfo(
                component,
                PackageManager.ComponentInfoFlags.of(0),
            )
        } else {
            activity.packageManager.getReceiverInfo(component, 0)
        }
        info.enabled && info.applicationInfo.enabled
    } catch (_: PackageManager.NameNotFoundException) {
        false
    }

    @Command
    fun health(invoke: Invoke) {
        val ret = JSObject()
        ret.put("platformSupported", true)
        ret.put("receiverRegistered", startupReceiverRegistered)
        ret.put("bootReceiverRegistered", startupBootReceiverRegistered)
        ret.put("exact", AlarmScheduler.canExact(activity.applicationContext))
        ret.put(
            "notificationsEnabled",
            NotificationManagerCompat.from(activity.applicationContext).areNotificationsEnabled(),
        )
        invoke.resolve(ret)
    }

    @Command
    fun sync(invoke: Invoke) {
        if (!startupReceiverRegistered) {
            invoke.reject("AlarmReceiver is missing or disabled in the installed manifest")
            return
        }
        val args = invoke.parseArgs(SyncArgs::class.java)
        val items = args.alarms.map {
            AlarmScheduler.Item(it.id, it.atMs, it.title, it.body)
        }
        val exact = AlarmScheduler.replaceAll(activity.applicationContext, items)
        val ret = JSObject()
        ret.put("exact", exact)
        invoke.resolve(ret)
    }
}
