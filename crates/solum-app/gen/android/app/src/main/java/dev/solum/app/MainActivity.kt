package dev.solum.app

import android.Manifest
import android.content.ContentUris
import android.os.Bundle
import android.content.Intent
import android.content.pm.PackageManager
import android.provider.CalendarContract
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private val shareInbox = ArrayDeque<JSONObject>()

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    captureShareIntent(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    captureShareIntent(intent)
  }

  /**
   * Accept only bounded text shares. The queue is deliberately in memory:
   * receiving an Android intent is not user consent to persist its contents.
   */
  @Synchronized
  private fun captureShareIntent(incoming: Intent?) {
    if (incoming?.action != Intent.ACTION_SEND) return
    val mime = incoming.type ?: return
    if (mime != "text/plain" && mime != "text/html") return
    val raw = incoming.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim().orEmpty()
    if (raw.isEmpty()) return
    val text = boundedCodePoints(raw, 4000)
    val title = boundedCodePoints(
      incoming.getCharSequenceExtra(Intent.EXTRA_SUBJECT)?.toString()?.trim().orEmpty(),
      48,
    )
    shareInbox.addLast(JSONObject().apply {
      put("title", title)
      put("text", text)
    })
    while (shareInbox.size > 20) shareInbox.removeFirst()
  }

  @Synchronized
  fun drainShareInboxJson(): String {
    val array = JSONArray()
    while (shareInbox.isNotEmpty()) array.put(shareInbox.removeFirst())
    return array.toString()
  }

  private fun boundedCodePoints(value: String, max: Int): String {
    val count = value.codePointCount(0, value.length)
    if (count <= max) return value
    val end = value.offsetByCodePoints(0, max)
    return value.substring(0, end) + "…"
  }

  fun hasCalendarReadPermission(): Boolean =
    ContextCompat.checkSelfPermission(this, Manifest.permission.READ_CALENDAR) ==
      PackageManager.PERMISSION_GRANTED

  /** Request only read access. The result is observed after the activity resumes. */
  fun requestCalendarReadPermission(): Boolean {
    if (hasCalendarReadPermission()) return true
    requestPermissions(arrayOf(Manifest.permission.READ_CALENDAR), 2201)
    return false
  }

  /** Read a bounded time window. Nothing is persisted by this method. */
  fun calendarCandidatesJson(startMillis: Long, endMillis: Long, limit: Int): String {
    if (!hasCalendarReadPermission()) return "[]"
    val builder = CalendarContract.Instances.CONTENT_URI.buildUpon()
    ContentUris.appendId(builder, startMillis)
    ContentUris.appendId(builder, endMillis)
    val projection = arrayOf(
      CalendarContract.Instances.EVENT_ID,
      CalendarContract.Instances.TITLE,
      CalendarContract.Instances.BEGIN,
      CalendarContract.Instances.END,
      CalendarContract.Instances.EVENT_LOCATION,
      CalendarContract.Instances.CALENDAR_DISPLAY_NAME,
      CalendarContract.Instances.ALL_DAY,
    )
    val output = JSONArray()
    contentResolver.query(
      builder.build(), projection, null, null,
      CalendarContract.Instances.BEGIN + " ASC",
    )?.use { cursor ->
      val boundedLimit = limit.coerceIn(1, 100)
      while (cursor.moveToNext() && output.length() < boundedLimit) {
        val eventId = cursor.getLong(0)
        val begin = cursor.getLong(2)
        output.put(JSONObject().apply {
          put("external_id", "android-calendar:$eventId:$begin")
          put("title", cursor.getString(1).orEmpty())
          put("begin_ms", begin)
          put("end_ms", cursor.getLong(3))
          put("location", cursor.getString(4).orEmpty())
          put("calendar", cursor.getString(5).orEmpty())
          put("all_day", cursor.getInt(6) != 0)
        })
      }
    }
    return output.toString()
  }

  /**
   * Open Android's own insert screen. Solum never requests WRITE_CALENDAR;
   * the calendar provider writes only after the user confirms there.
   */
  fun openCalendarInsert(
    title: String,
    startMillis: Long,
    endMillis: Long,
    location: String,
    description: String,
  ): Boolean {
    val insert = Intent(Intent.ACTION_INSERT).setData(CalendarContract.Events.CONTENT_URI)
      .putExtra(CalendarContract.Events.TITLE, boundedCodePoints(title, 200))
      .putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, startMillis)
      .putExtra(CalendarContract.Events.EVENT_LOCATION, boundedCodePoints(location, 300))
      .putExtra(CalendarContract.Events.DESCRIPTION, boundedCodePoints(description, 1000))
    if (endMillis > startMillis) insert.putExtra(CalendarContract.EXTRA_EVENT_END_TIME, endMillis)
    return if (insert.resolveActivity(packageManager) != null) {
      startActivity(insert)
      true
    } else {
      false
    }
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
