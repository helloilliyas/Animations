package lk.personal.csetrader

import android.Manifest
import android.annotation.SuppressLint
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONObject

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  /**
   * Exposes `window.CseBackgroundPort` to the app's own page only (other
   * origins, such as the embedded TradingView chart, cannot use it).
   * Messages are JSON: {id, cmd, ...}; replies are {id, result}.
   */
  @SuppressLint("RequiresFeature")
  override fun onWebViewCreate(webView: WebView) {
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return
    val origins = setOf("http://tauri.localhost", "https://tauri.localhost")
    WebViewCompat.addWebMessageListener(webView, "CseBackgroundPort", origins) { _, message, _, isMainFrame, reply ->
      if (!isMainFrame) return@addWebMessageListener
      val m = try { JSONObject(message.data ?: "{}") } catch (_: Exception) { return@addWebMessageListener }
      val result: Any = try { when (m.optString("cmd")) {
        "start" -> {
          requestNotificationPermission()
          requestBatteryExemption()
          AlertService.start(this, m.optString("path"))
          true
        }
        "stop" -> { AlertService.stop(this); true }
        "isRunning" -> AlertService.running
        // False after the user tapped Stop on the notification.
        "wanted" -> getSharedPreferences(AlertService.PREFS, MODE_PRIVATE).getBoolean("enabled", false)
        // Timed checks: minutes 15, 30 or 60; 0 turns them off.
        "schedule" -> {
          val minutes = m.optInt("minutes")
          Outbox.setPath(this, m.optString("path"))
          if (minutes > 0) {
            requestNotificationPermission()
            requestBatteryExemption()
          }
          CheckJob.schedule(this, minutes)
          true
        }
        // Posts notifications for new outbox lines (alerts found while the app is open).
        "drain" -> {
          requestNotificationPermission()
          Outbox.setPath(this, m.optString("path"))
          Outbox.drain(this)
          true
        }
        "notify" -> {
          requestNotificationPermission()
          Notifier.alert(this, 10_000 + m.optInt("nid"), m.optString("title"), m.optString("body"))
          true
        }
        // Shows (and clears) the crash saved by CrashCatcher.
        "lastCrash" -> {
          val f = java.io.File(filesDir, "last-crash.txt")
          if (f.exists()) f.readText().also { f.delete() } else ""
        }
        else -> false
      } } catch (e: Exception) {
        "error: $e"
      }
      reply.postMessage(JSONObject().put("id", m.optInt("id")).put("result", result).toString())
    }
  }

  private fun requestNotificationPermission() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
      ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) !=
      PackageManager.PERMISSION_GRANTED
    ) {
      runOnUiThread {
        ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 41)
      }
    }
  }

  /** Asks once to run without battery restrictions, so alerts keep coming. */
  @SuppressLint("BatteryLife")
  private fun requestBatteryExemption() {
    val prefs = getSharedPreferences(AlertService.PREFS, MODE_PRIVATE)
    if (prefs.getBoolean("asked_battery", false)) return
    prefs.edit().putBoolean("asked_battery", true).apply()
    val pm = getSystemService(POWER_SERVICE) as PowerManager
    if (pm.isIgnoringBatteryOptimizations(packageName)) return
    runOnUiThread {
      try {
        startActivity(
          Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
        )
      } catch (_: Exception) {
      }
    }
  }
}
