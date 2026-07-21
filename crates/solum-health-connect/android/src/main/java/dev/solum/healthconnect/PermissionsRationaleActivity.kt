package dev.solum.healthconnect

import android.app.Activity
import android.os.Bundle
import android.widget.TextView

/**
 * Health Connect's grant screen links to this for "why does this app want
 * this data" (androidx.health.ACTION_SHOW_PERMISSIONS_RATIONALE /
 * VIEW_PERMISSION_USAGE — see AndroidManifest.xml). Not declaring it isn't
 * merely a Play Store review requirement: tapping that link on a sideloaded
 * install with no rationale activity registered crashes (see
 * docs/PITFALLS.md), so this stays even though Solum isn't Play-distributed.
 */
class PermissionsRationaleActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(TextView(this).apply {
            text = "Solum 读取心率 / 步数 / 睡眠数据仅用于本机记录（可随时在「记忆台账」里查看或删除），不上传云端，不与第三方共享。"
            setPadding(48, 96, 48, 48)
            textSize = 16f
        })
    }
}
