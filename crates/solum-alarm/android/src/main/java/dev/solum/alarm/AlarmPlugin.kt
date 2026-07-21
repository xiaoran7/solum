package dev.solum.alarm

import android.app.Activity
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
    @Command
    fun sync(invoke: Invoke) {
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
