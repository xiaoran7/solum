package dev.solum.app

import android.os.Bundle
import android.content.Intent
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  /** Relaunch the Tauri process after an account/profile boundary change. */
  fun restartApp() {
    val launchIntent = packageManager.getLaunchIntentForPackage(packageName) ?: return
    val component = launchIntent.component ?: return
    val restartIntent = Intent.makeRestartActivityTask(component)
    startActivity(restartIntent)
    Runtime.getRuntime().exit(0)
  }
}
