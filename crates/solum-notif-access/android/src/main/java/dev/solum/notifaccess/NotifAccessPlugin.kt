package dev.solum.notifaccess

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.core.app.NotificationManagerCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * F1 companion: Solum's notification-capture pipeline (`dev.solum.app.SolumNotificationListener`)
 * needs the user to grant "notification access" — a system permission with
 * no runtime-request dialog, only a Settings deep link. This plugin does
 * exposes the listener state and settings deep link, plus the deliberately
 * narrow installed-app picker and foreground-pipeline controls needed by F20.
 */
@TauriPlugin
class NotifAccessPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun isEnabled(invoke: Invoke) {
        val enabled = NotificationManagerCompat
            .getEnabledListenerPackages(activity)
            .contains(activity.packageName)
        val ret = JSObject()
        ret.put("enabled", enabled)
        invoke.resolve(ret)
    }

    @Command
    fun openSettings(invoke: Invoke) {
        activity.startActivity(Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS"))
        invoke.resolve()
    }

    /**
     * Return visible, launchable installed applications for Solum's source picker.
     * The package identifier is returned only so the Rust policy can make the
     * correct listener decision; the WebView presents the label, never asks a
     * person to type or recognize a package name. Android may scope this list
     * under its package-visibility rules, which is safer than requesting the
     * store-sensitive QUERY_ALL_PACKAGES permission.
     */
    @Command
    fun installedApps(invoke: Invoke) {
        val pm = activity.packageManager
        val launcherIntent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val items = pm.queryIntentActivities(launcherIntent, PackageManager.MATCH_ALL)
            .asSequence()
            .map { it.activityInfo.applicationInfo }
            .filter { it.packageName != activity.packageName }
            .distinctBy { it.packageName }
            .map { appInfo ->
                JSObject().apply {
                    put("name", pm.getApplicationLabel(appInfo).toString().ifBlank { appInfo.packageName })
                    put("packageName", appInfo.packageName)
                }
            }
            .sortedBy { it.getString("name")?.lowercase() ?: "" }
            .toList()
        val ret = JSObject()
        ret.put("apps", items)
        invoke.resolve(ret)
    }

    @Command
    fun pipelineStatus(invoke: Invoke) {
        val power = activity.getSystemService(PowerManager::class.java)
        val ret = JSObject()
        ret.put("running", NotificationPipelineService.running)
        ret.put("ignoringBatteryOptimizations", power.isIgnoringBatteryOptimizations(activity.packageName))
        invoke.resolve(ret)
    }

    @Command
    fun startPipeline(invoke: Invoke) {
        NotificationPipelineService.start(activity.applicationContext)
        invoke.resolve()
    }

    @Command
    fun stopPipeline(invoke: Invoke) {
        NotificationPipelineService.stop(activity.applicationContext)
        invoke.resolve()
    }

    @Command
    fun requestIgnoreBatteryOptimizations(invoke: Invoke) {
        val power = activity.getSystemService(PowerManager::class.java)
        if (!power.isIgnoringBatteryOptimizations(activity.packageName)) {
            activity.startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                data = Uri.parse("package:${activity.packageName}")
            })
        }
        invoke.resolve()
    }

    @Command
    fun openBatterySettings(invoke: Invoke) {
        activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
        invoke.resolve()
    }

    /** There is no stable cross-ROM auto-start intent. App details is honest
     * and lets MIUI/EMUI/etc. expose their own per-app background controls. */
    @Command
    fun openAppBackgroundSettings(invoke: Invoke) {
        activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
            data = Uri.parse("package:${activity.packageName}")
        })
        invoke.resolve()
    }
}
