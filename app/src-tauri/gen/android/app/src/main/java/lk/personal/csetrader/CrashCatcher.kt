package lk.personal.csetrader

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import java.io.File

/**
 * Starts before anything else in the app process and records crashes
 * (Kotlin exceptions here, Rust panics via the hook in `src/lib.rs`). The
 * details are shown as a notification on the next start and in the app.
 */
class CrashCatcher : ContentProvider() {
    override fun onCreate(): Boolean {
        val ctx = context ?: return true
        val crash = File(ctx.filesDir, "last-crash.txt")
        val panic = File(ctx.filesDir, "rust-panic.txt")
        try {
            if (panic.exists()) {
                crash.writeText(panic.readText())
                panic.delete()
            }
            if (crash.exists()) {
                val text = crash.readText()
                Notifier.alert(ctx, 7999, "CSE Trader crashed last time", text.take(1500))
            }
        } catch (_: Throwable) {
        }
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, e ->
            try {
                crash.writeText("${java.util.Date()}\n${android.util.Log.getStackTraceString(e)}")
            } catch (_: Throwable) {
            }
            previous?.uncaughtException(thread, e)
        }
        return true
    }

    override fun query(u: Uri, p: Array<out String>?, s: String?, a: Array<out String>?, o: String?): Cursor? = null
    override fun getType(uri: Uri): String? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, s: String?, a: Array<out String>?): Int = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<out String>?): Int = 0
}
