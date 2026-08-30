package dev.solum.app

import android.Manifest
import android.app.Activity
import android.content.ContentUris
import android.os.Bundle
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.CalendarContract
import android.speech.RecognizerIntent
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.chinese.ChineseTextRecognizerOptions
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : TauriActivity() {
  private val shareInbox = ArrayDeque<JSONObject>()
  private val captureNotices = ArrayDeque<String>()

  companion object {
    private const val VOICE_REQUEST = 2202
    private const val OCR_PICK_REQUEST = 2203
    private const val MAX_IMAGE_BYTES = 20L * 1024L * 1024L
  }

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

  /** Receiving a share is not consent to persist it; only bounded text enters RAM. */
  @Synchronized
  private fun captureShareIntent(incoming: Intent?) {
    if (incoming?.action != Intent.ACTION_SEND) return
    val mime = incoming.type ?: return
    if (mime.startsWith("image/")) {
      val uri = sharedStreamUri(incoming)
      if (uri == null) noteCapture("分享的图片无法读取，请改用“选择截图 OCR”。") else recognizeImage(uri)
      return
    }
    if (mime != "text/plain" && mime != "text/html") return
    val raw = incoming.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim().orEmpty()
    if (raw.isEmpty()) return
    val text = boundedCodePoints(raw, 4000)
    val title = boundedCodePoints(
      incoming.getCharSequenceExtra(Intent.EXTRA_SUBJECT)?.toString()?.trim().orEmpty(),
      48,
    )
    enqueueCapture("share", title, text)
  }

  @Synchronized
  fun drainShareInboxJson(): String {
    val items = JSONArray()
    while (shareInbox.isNotEmpty()) items.put(shareInbox.removeFirst())
    val notices = JSONArray()
    while (captureNotices.isNotEmpty()) notices.put(captureNotices.removeFirst())
    return JSONObject().put("items", items).put("notices", notices).toString()
  }

  @Synchronized
  private fun enqueueCapture(source: String, title: String, text: String) {
    val bounded = boundedCodePoints(text.trim(), 4000)
    if (bounded.isEmpty()) return
    shareInbox.addLast(JSONObject().apply {
      put("source", source)
      put("title", boundedCodePoints(title.trim(), 48))
      put("text", bounded)
    })
    while (shareInbox.size > 20) shareInbox.removeFirst()
  }

  @Synchronized
  private fun noteCapture(message: String) {
    captureNotices.addLast(boundedCodePoints(message, 160))
    while (captureNotices.size > 8) captureNotices.removeFirst()
  }

  @Suppress("DEPRECATION")
  private fun sharedStreamUri(incoming: Intent): Uri? =
    if (android.os.Build.VERSION.SDK_INT >= 33) {
      incoming.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        ?: incoming.clipData?.getItemAt(0)?.uri
    } else {
      incoming.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
        ?: incoming.clipData?.getItemAt(0)?.uri
    }

  /** Opens the system speech UI. Solum itself never records or stores audio. */
  fun startVoiceCapture(): Boolean {
    val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
      putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
      putExtra(RecognizerIntent.EXTRA_LANGUAGE, "zh-CN")
      putExtra(RecognizerIntent.EXTRA_PROMPT, "说出要交给 Solum 的内容")
      putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
    }
    if (intent.resolveActivity(packageManager) == null) return false
    @Suppress("DEPRECATION")
    startActivityForResult(intent, VOICE_REQUEST)
    return true
  }

  /** User-initiated document picker; URI permission is temporary and sufficient for OCR. */
  fun startOcrPicker(): Boolean {
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "image/*"
    }
    if (intent.resolveActivity(packageManager) == null) return false
    @Suppress("DEPRECATION")
    startActivityForResult(intent, OCR_PICK_REQUEST)
    return true
  }

  @Deprecated("Activity result bridge is retained for the generated Tauri activity.")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    if (resultCode != Activity.RESULT_OK) return
    when (requestCode) {
      VOICE_REQUEST -> {
        val text = data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)
          ?.firstOrNull()?.trim().orEmpty()
        if (text.isEmpty()) noteCapture("系统语音识别没有返回文字。")
        else enqueueCapture("manual", "语音输入", text)
      }
      OCR_PICK_REQUEST -> data?.data?.let { recognizeImage(it) }
    }
  }

  /** Bundled ML Kit model: image and OCR output stay in this process. */
  private fun recognizeImage(uri: Uri) {
    val bytes = try {
      contentResolver.openAssetFileDescriptor(uri, "r")?.use { it.length }
    } catch (_: Exception) { null }
    if (bytes != null && bytes > MAX_IMAGE_BYTES) {
      noteCapture("图片超过 20 MiB，未执行 OCR。")
      return
    }
    val image = try {
      InputImage.fromFilePath(this, uri)
    } catch (_: Exception) {
      noteCapture("图片无法解码，未执行 OCR。")
      return
    }
    val recognizer = TextRecognition.getClient(ChineseTextRecognizerOptions.Builder().build())
    recognizer.process(image)
      .addOnSuccessListener { result ->
        val text = result.text.trim()
        if (text.isEmpty()) noteCapture("截图中没有识别到文字。")
        else enqueueCapture("share", "截图 OCR", text)
      }
      .addOnFailureListener { noteCapture("截图 OCR 暂时失败，请稍后重试。") }
      .addOnCompleteListener { recognizer.close() }
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

  /**
   * Debug-only fault injection for reminder QA. Unlike `am force-stop`, this
   * kills only the current process, so AlarmManager keeps already registered
   * PendingIntents and can prove receiver delivery after process death.
   */
  fun debugScheduleProcessDeath(): Boolean {
    if (!BuildConfig.DEBUG) return false
    android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
      android.os.Process.killProcess(android.os.Process.myPid())
    }, 600)
    return true
  }

  /** Open an external URL in the system browser (e.g. for OAuth 2.0 authorization). */
  fun openExternalUrl(url: String): Boolean {
    val parsed = try { Uri.parse(url) } catch (_: Exception) { return false }
    val intent = Intent(Intent.ACTION_VIEW, parsed).apply {
      addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    return try {
      startActivity(intent)
      true
    } catch (_: Exception) {
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
