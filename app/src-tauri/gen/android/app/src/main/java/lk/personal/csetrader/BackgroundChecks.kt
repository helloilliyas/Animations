package lk.personal.csetrader

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
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

/**
 * Timed background checks (every 15, 30 or 60 minutes) with Android's
 * JobScheduler. Runs the Rust check during trading hours on weekdays,
 * plus once after the close.
 */
class CheckJob : JobService() {
    companion object {
        private const val JOB_ID = 7101

        /** 0 cancels the checks. */
        fun schedule(ctx: Context, minutes: Int) {
            val js = ctx.getSystemService(JobScheduler::class.java)
            if (minutes <= 0) {
                js.cancel(JOB_ID)
                return
            }
            val period = minutes * 60_000L
            val existing = js.getPendingJob(JOB_ID)
            if (existing != null && existing.intervalMillis == period) return
            val job = JobInfo.Builder(JOB_ID, ComponentName(ctx, CheckJob::class.java))
                .setPeriodic(period)
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setPersisted(true)
                .build()
            js.schedule(job)
        }

        /** Whether a check is due now (Sri Lanka time). */
        private fun due(ctx: Context): Pair<Boolean, Boolean> {
            val prefs = ctx.getSharedPreferences(AlertService.PREFS, Context.MODE_PRIVATE)
            val now = Calendar.getInstance(TimeZone.getTimeZone("Asia/Colombo"))
            val day = now.get(Calendar.DAY_OF_WEEK)
            if (day == Calendar.SATURDAY || day == Calendar.SUNDAY) return false to false
            val minutes = now.get(Calendar.HOUR_OF_DAY) * 60 + now.get(Calendar.MINUTE)
            val today = "%d-%d".format(now.get(Calendar.YEAR), now.get(Calendar.DAY_OF_YEAR))
            val trading = minutes in (9 * 60 + 25)..(14 * 60 + 30)
            val afterClose = minutes in (14 * 60 + 31)..(17 * 60 + 30) &&
                prefs.getString("closing_check", null) != today
            if (afterClose) prefs.edit().putString("closing_check", today).apply()
            return (trading || afterClose) to afterClose
        }
    }

    override fun onStartJob(params: JobParameters): Boolean {
        // Live mode checks continuously on its own.
        if (AlertService.running || !due(this).first) return false
        val outbox = Outbox.path(this) ?: return false
        val db = File(File(outbox).parentFile, "cse-trader.sqlite")
        if (!db.exists()) return false
        Thread {
            val n = try {
                NativeCheck.runCheck(db.absolutePath)
            } catch (_: Throwable) {
                -1
            }
            if (n >= 0) Outbox.drain(this)
            jobFinished(params, false)
        }.start()
        return true
    }

    override fun onStopJob(params: JobParameters): Boolean = false
}
