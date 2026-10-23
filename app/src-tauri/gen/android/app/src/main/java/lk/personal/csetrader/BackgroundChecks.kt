package lk.personal.csetrader

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.app.Service
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PersistableBundle
import android.os.PowerManager
import androidx.core.app.ServiceCompat
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.util.Calendar
import java.util.TimeZone

/** The Rust check in libcse_trader_lib (see `src/lib.rs`). */
object NativeCheck {
    init {
        System.loadLibrary("cse_trader_lib")
    }

    /** Returns the number of new alerts, or -1 on failure. */
    @JvmStatic
    external fun runCheck(dbPath: String): Int
}

/**
 * Alerts outbox written by the Rust service (one JSON line per check).
 * Every notification goes through here, so nothing is posted twice.
 */
object Outbox {
    private fun prefs(ctx: Context) = ctx.getSharedPreferences(AlertService.PREFS, Context.MODE_PRIVATE)

    /** Remembers the outbox; starts reading from its current end the first time. */
    @Synchronized
    fun setPath(ctx: Context, path: String) {
        if (path.isEmpty()) return
        val p = prefs(ctx)
        if (p.getString("outbox", null) == path && p.contains("offset")) return
        p.edit().putString("outbox", path).putLong("offset", File(path).length()).apply()
    }

    fun path(ctx: Context): String? = prefs(ctx).getString("outbox", null)

    /** Posts a notification for each new line. */
    @Synchronized
    fun drain(ctx: Context) {
        val p = prefs(ctx)
        val f = path(ctx)?.let { File(it) } ?: return
        if (!f.exists()) return
        var offset = p.getLong("offset", 0L)
        if (f.length() < offset) offset = 0L // file was replaced
        if (f.length() == offset) return
        try {
            RandomAccessFile(f, "r").use { raf ->
                raf.seek(offset)
                while (true) {
                    val raw = raf.readLine() ?: break
                    val line = String(raw.toByteArray(Charsets.ISO_8859_1), Charsets.UTF_8)
                    if (line.isBlank()) continue
                    val o = JSONObject(line)
                    Notifier.alert(ctx, 10_000 + o.optInt("id"), o.optString("title"), o.optString("body"))
                }
                offset = raf.filePointer
            }
        } catch (_: Exception) {
            offset = f.length()
        }
        p.edit().putLong("offset", offset).apply()
    }
}

/** Market hours used by the background checks. */
object Sessions {
    private fun minutesIn(zone: String): Pair<Boolean, Int> {
        val now = Calendar.getInstance(TimeZone.getTimeZone(zone))
        val day = now.get(Calendar.DAY_OF_WEEK)
        val weekday = day != Calendar.SATURDAY && day != Calendar.SUNDAY
        return weekday to now.get(Calendar.HOUR_OF_DAY) * 60 + now.get(Calendar.MINUTE)
    }

    /** US regular session, 09:30–16:00 New York time on weekdays. */
    fun usOpen(): Boolean {
        val (weekday, m) = minutesIn("America/New_York")
        return weekday && m in (9 * 60 + 30) until (16 * 60)
    }

    /**
     * Whether a check is due for `market` ("cse" or "us"): during its
     * session, or once in the three hours after its close.
     */
    fun due(ctx: Context, market: String): Boolean {
        val prefs = ctx.getSharedPreferences(AlertService.PREFS, Context.MODE_PRIVATE)
        val us = market == "us"
        val zone = if (us) "America/New_York" else "Asia/Colombo"
        val open = 9 * 60 + 30
        val close = if (us) 16 * 60 else 14 * 60 + 30
        val flag = if (us) "closing_check_us" else "closing_check"
        val (weekday, minutes) = minutesIn(zone)
        if (!weekday) return false
        if (minutes in (open - 5)..close) return true
        val now = Calendar.getInstance(TimeZone.getTimeZone(zone))
        val today = "%d-%d".format(now.get(Calendar.YEAR), now.get(Calendar.DAY_OF_YEAR))
        val afterClose = minutes in (close + 1)..(close + 180) && prefs.getString(flag, null) != today
        if (afterClose) prefs.edit().putString(flag, today).apply()
        return afterClose
    }

    /** Runs the Rust check on this thread and posts any new alerts. */
    fun runCheck(ctx: Context): Boolean {
        val outbox = Outbox.path(ctx) ?: return false
        val db = File(File(outbox).parentFile, "cse-trader.sqlite")
        if (!db.exists()) return false
        val n = try {
            NativeCheck.runCheck(db.absolutePath)
        } catch (_: Throwable) {
            -1
        }
        if (n >= 0) Outbox.drain(ctx)
        return n >= 0
    }
}

/**
 * Timed background checks with Android's JobScheduler, one job per market:
 * CSE every 15, 30 or 60 minutes in Sri Lanka trading hours, and US every
 * 15, 30 or 60 minutes in New York trading hours (each plus one check
 * after its close). US checks every 10 or 5 minutes use [UsWatchService],
 * which a 15-minute job starts at the US open.
 */
class CheckJob : JobService() {
    companion object {
        private const val JOB_CSE = 7101
        private const val JOB_US = 7102
        private const val EXTRA_MARKET = "market"

        /** CSE checks; 0 cancels them. */
        fun schedule(ctx: Context, minutes: Int) = scheduleJob(ctx, JOB_CSE, "cse", minutes)

        /** US checks: 0 off, 60/30/15 as jobs, 10/5 with the US watch service. */
        fun scheduleUs(ctx: Context, minutes: Int) {
            ctx.getSharedPreferences(AlertService.PREFS, Context.MODE_PRIVATE)
                .edit().putInt("us_minutes", minutes).apply()
            val watch = minutes in 1..14
            if (!watch) UsWatchService.stop(ctx)
            scheduleJob(ctx, JOB_US, "us", if (minutes <= 0) 0 else maxOf(minutes, 15))
            if (watch) UsWatchService.start(ctx)
        }

        fun usWatchMinutes(ctx: Context): Int =
            ctx.getSharedPreferences(AlertService.PREFS, Context.MODE_PRIVATE)
                .getInt("us_minutes", 15).takeIf { it in 1..14 } ?: 0

        private fun scheduleJob(ctx: Context, id: Int, market: String, minutes: Int) {
            val js = ctx.getSystemService(JobScheduler::class.java)
            if (minutes <= 0) {
                js.cancel(id)
                return
            }
            val period = minutes * 60_000L
            val existing = js.getPendingJob(id)
            if (existing != null && existing.intervalMillis == period) return
            val job = JobInfo.Builder(id, ComponentName(ctx, CheckJob::class.java))
                .setPeriodic(period)
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setPersisted(true)
                .setExtras(PersistableBundle().apply { putString(EXTRA_MARKET, market) })
                .build()
            js.schedule(job)
        }
    }

    override fun onStartJob(params: JobParameters): Boolean {
        val market = params.extras.getString(EXTRA_MARKET) ?: "cse"
        // Live mode checks the CSE continuously on its own.
        if (market == "cse" && AlertService.running) return false
        if (!Sessions.due(this, market)) return false
        if (market == "us" && usWatchMinutes(this) > 0 && Sessions.usOpen()) {
            UsWatchService.start(this)
            if (UsWatchService.running) return false
        }
        Thread {
            Sessions.runCheck(this)
            jobFinished(params, false)
        }.start()
        return true
    }

    override fun onStopJob(params: JobParameters): Boolean = false
}

/**
 * US checks every 10 or 5 minutes: runs only during the US session, keeps
 * the phone awake with a notification showing, and stops after the close
 * (the after-close check is the job's).
 */
class UsWatchService : Service() {
    companion object {
        private const val NOTIFICATION_ID = 7002

        @Volatile
        var running = false
            private set

        fun start(ctx: Context) {
            if (running || !Sessions.usOpen() || CheckJob.usWatchMinutes(ctx) == 0) return
            try {
                val i = Intent(ctx, UsWatchService::class.java)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) ctx.startForegroundService(i)
                else ctx.startService(i)
            } catch (_: Exception) {
                // Not allowed from the background (no battery exemption): the
                // 15-minute job keeps checking instead.
            }
        }

        fun stop(ctx: Context) {
            ctx.stopService(Intent(ctx, UsWatchService::class.java))
        }
    }

    private var worker: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val minutes = CheckJob.usWatchMinutes(this)
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
        ServiceCompat.startForeground(this, NOTIFICATION_ID, Notifier.usWatching(this, minutes), type)
        if (minutes == 0 || !Sessions.usOpen()) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (!running) {
            running = true
            wakeLock = (getSystemService(POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "csetrader:us")
                .apply { acquire(7 * 60 * 60 * 1000L) }
            worker = Thread {
                try {
                    while (running && Sessions.usOpen()) {
                        Sessions.runCheck(this)
                        Thread.sleep(CheckJob.usWatchMinutes(this).coerceAtLeast(5) * 60_000L)
                    }
                } catch (_: InterruptedException) {
                }
                stopSelf()
            }.apply { start() }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        worker?.interrupt()
        if (wakeLock?.isHeld == true) wakeLock?.release()
        super.onDestroy()
    }
}
