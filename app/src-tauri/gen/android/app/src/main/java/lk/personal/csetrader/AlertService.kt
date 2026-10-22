package lk.personal.csetrader

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import java.util.Calendar
import java.util.TimeZone

/** Posts alert notifications; shared by the service and the in-app bridge. */
object Notifier {
    const val CHANNEL_ALERTS = "alerts"
    const val CHANNEL_WATCH = "watch"

    fun ensureChannels(ctx: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_ALERTS, "Price alerts", NotificationManager.IMPORTANCE_HIGH)
                .apply { description = "Signals and price levels for your favourites" }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_WATCH, "Background mode", NotificationManager.IMPORTANCE_LOW)
                .apply { description = "Shown while CSE Trader watches prices in the background" }
        )
    }

    private fun openApp(ctx: Context): PendingIntent =
        PendingIntent.getActivity(
            ctx, 0,
            Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

    fun alert(ctx: Context, id: Int, title: String, body: String) {
        ensureChannels(ctx)
        val n = NotificationCompat.Builder(ctx, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_alert)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(openApp(ctx))
            .build()
        try {
            ctx.getSystemService(NotificationManager::class.java).notify(id, n)
        } catch (_: SecurityException) {
            // Notification permission not granted.
        }
    }

    fun watching(ctx: Context): Notification {
        ensureChannels(ctx)
        val stop = PendingIntent.getService(
            ctx, 1,
            Intent(ctx, AlertService::class.java).setAction(AlertService.ACTION_STOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        return NotificationCompat.Builder(ctx, CHANNEL_WATCH)
            .setSmallIcon(R.drawable.ic_stat_alert)
            .setContentTitle("CSE Trader alerts are on")
            .setContentText("Watching your favourites during market hours")
            .setOngoing(true)
            .setContentIntent(openApp(ctx))
            .addAction(0, "Stop", stop)
            .build()
    }
}

/**
 * Background mode. Keeps the app process (and its Rust price poller)
 * alive, holds a CPU wake lock only during CSE trading hours, and turns
 * new lines in the alerts outbox file into notifications.
 */
class AlertService : Service() {
    companion object {
        const val ACTION_STOP = "lk.personal.csetrader.STOP_ALERTS"
        const val EXTRA_OUTBOX = "outbox"
        const val PREFS = "background"
        private const val NOTIFICATION_ID = 7001
        private const val TICK_MS = 20_000L

        @Volatile
        var running = false
            private set

        fun start(ctx: Context, outbox: String) {
            val i = Intent(ctx, AlertService::class.java).putExtra(EXTRA_OUTBOX, outbox)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) ctx.startForegroundService(i)
            else ctx.startService(i)
        }

        fun stop(ctx: Context) {
            ctx.startService(Intent(ctx, AlertService::class.java).setAction(ACTION_STOP))
        }
    }

    private val handler = Handler(Looper.getMainLooper())
    private var wakeLock: PowerManager.WakeLock? = null
    private val prefs by lazy { getSharedPreferences(PREFS, MODE_PRIVATE) }

    private val tick = object : Runnable {
        override fun run() {
            Outbox.drain(this@AlertService)
            manageWakeLock()
            handler.postDelayed(this, TICK_MS)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            prefs.edit().putBoolean("enabled", false).apply()
            shutdown()
            return START_NOT_STICKY
        }
        intent?.getStringExtra(EXTRA_OUTBOX)?.let { Outbox.setPath(this, it) }
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, Notifier.watching(this), type)
        prefs.edit().putBoolean("enabled", true).apply()
        if (!running) {
            running = true
            handler.post(tick)
        }
        return START_STICKY
    }

    /** CPU stays awake 09:25–14:40 Sri Lanka time on weekdays only. */
    private fun manageWakeLock() {
        val now = Calendar.getInstance(TimeZone.getTimeZone("Asia/Colombo"))
        val day = now.get(Calendar.DAY_OF_WEEK)
        val minutes = now.get(Calendar.HOUR_OF_DAY) * 60 + now.get(Calendar.MINUTE)
        val trading = day != Calendar.SATURDAY && day != Calendar.SUNDAY &&
            minutes in (9 * 60 + 25)..(14 * 60 + 40)
        val held = wakeLock?.isHeld == true
        if (trading && !held) {
            wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "csetrader:alerts")
                .apply { acquire(6 * 60 * 60 * 1000L) }
        } else if (!trading && held) {
            wakeLock?.release()
        }
    }

    private fun shutdown() {
        running = false
        handler.removeCallbacks(tick)
        if (wakeLock?.isHeld == true) wakeLock?.release()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        handler.removeCallbacks(tick)
        if (wakeLock?.isHeld == true) wakeLock?.release()
        super.onDestroy()
    }
}
