package dev.solum.alarm

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Alarms don't survive reboot: re-arm every future one from the persisted set. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            AlarmScheduler.applyAll(ctx)
        }
    }
}
